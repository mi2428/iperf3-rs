use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const COMPOSE_FILE: &str = "docker-compose.test.yml";
const PUSHGATEWAY_URL: &str = "http://pushgateway:9091";
const PUSH_JOB: &str = "integration";
const PUSH_TEST: &str = "self";
const CLIENT_SCENARIO: &str = "tcp";
const FILE_SCENARIO: &str = "tcp-file";
const DELETE_SCENARIO: &str = "tcp-delete";
const SERVER_SCENARIO: &str = "tcp-server";
const LIVE_SCENARIO: &str = "tcp-live";
const WINDOW_SCENARIO: &str = "tcp-window";
const UDP_SCENARIO: &str = "udp";
const REVERSE_SCENARIO: &str = "tcp-reverse";
const BIDIR_SCENARIO: &str = "tcp-bidir";

// This integration test exercises the Docker Compose test topology end to end.
//
// The topology contains:
// - `server-rs`: an iperf3-rs server with Pushgateway defaults in its
//   environment, used to verify that the Rust frontend can accept upstream
//   iperf3 traffic and export server-role interval metrics.
// - `reference`: an upstream esnet/iperf3 server, used to verify that the
//   iperf3-rs client remains wire-compatible with the reference implementation.
// - `pushgateway`: a Prometheus Pushgateway instance, used to verify that
//   iperf3-rs can publish interval metrics during client and server runs.
// - `client-rs`: the test runner image, which contains both iperf3-rs and the
//   upstream iperf3 binary and has Pushgateway defaults in its environment so
//   the same container can drive all scenarios.
//
// The test succeeds only when all of the following are true:
// - the Compose image builds and the server services start;
// - Pushgateway reports readiness;
// - an upstream iperf3 client can complete a JSON test against iperf3-rs;
// - the metrics-enabled iperf3-rs server publishes non-zero
//   `iperf3_bytes` and `iperf3_bandwidth` samples with the expected
//   integration labels;
// - that same metrics-enabled server keeps the normal upstream human-readable
//   stdout shape instead of going silent;
// - an iperf3-rs client can complete a JSON test against upstream iperf3;
// - an iperf3-rs client with metrics enabled still preserves `-J` as a normal
//   complete JSON document rather than turning it into JSON stream output;
// - an iperf3-rs client run from the metrics-enabled `client-rs` service
//   publishes non-zero `iperf3_bytes` and `iperf3_bandwidth` samples with the
//   expected integration labels;
// - TCP runs expose sender-side TCP_INFO metrics while UDP-only metrics stay at
//   zero for TCP scenarios;
// - a metrics-enabled iperf3-rs client without `-J` keeps normal
//   human-readable stdout;
// - client metrics appear in Pushgateway while a longer run is still active;
// - CLI file metrics can be written in Prometheus format without replacing
//   normal iperf stdout, while Pushgateway export still receives the same run;
// - `--push.delete-on-exit` publishes live metrics during a run and removes the
//   retained Pushgateway group after the run exits;
// - `--push.interval` publishes aggregated `iperf3_window_*` metrics instead
//   of the immediate interval metric families for that grouping key;
// - the long-running iperf3-rs server continues pushing metrics after a later
//   client connection;
// - UDP, reverse TCP, and bidirectional TCP runs all complete and publish
//   protocol-specific metrics through the same environment-based Pushgateway path.
#[test]
#[ignore = "requires Docker"]
fn compose_interop_and_pushgateway_metrics() {
    let project = ComposeProject::new();

    // Build the shared test image and start the long-running services. The
    // `ComposeProject` uses a unique project name so concurrent or stale test
    // runs do not reuse containers from another invocation.
    if !truthy_env("SKIP_INTEGRATION_IMAGE_BUILD") {
        project.run_compose(&["build", "client-rs"]);
    }
    project.run_compose(&["up", "-d", "server-rs", "reference", "pushgateway"]);

    // Wait until Pushgateway is ready to accept writes. This prevents the
    // metrics assertion from racing the container startup path.
    wait_for("pushgateway readiness", || {
        project.client_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/-/ready")])
    });

    // Interop check 1: the upstream iperf3 client must be able to talk to the
    // iperf3-rs server and return a complete JSON summary containing traffic.
    let upstream_to_iperf3rs = retry_json_client("upstream client to iperf3-rs server", || {
        project.client_output(&["iperf3", "-c", "server-rs", "-t", "3", "-i", "1", "-J"])
    });
    assert_iperf_summary_has_traffic(&upstream_to_iperf3rs);

    // Server metrics check: because `server-rs` itself gets Pushgateway
    // configuration from its service environment, the upstream client traffic
    // above should leave a server-side metric group in Pushgateway. This keeps
    // the topology close to the real deployment shape instead of adding a
    // dedicated one-off metrics server.
    wait_for_pushgateway_metrics(
        &project,
        SERVER_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );
    assert_metric_absent(&project, "iperf3_udp_packets", SERVER_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_lost_packets", SERVER_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_jitter_seconds", SERVER_SCENARIO);
    assert_metric_absent(&project, "iperf3_tcp_rtt_seconds", SERVER_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_out_of_order_packets", SERVER_SCENARIO);
    assert_metric_value_eq(&project, "iperf3_omitted", SERVER_SCENARIO, 0.0);
    wait_for_service_log_contains(&project, "server-rs", "Server listening on");
    wait_for_service_log_contains(&project, "server-rs", "[ ID]");
    let first_server_push =
        wait_for_metric_value_gt(&project, "push_time_seconds", SERVER_SCENARIO, 0.0);

    // Interop check 2: the iperf3-rs client must be able to talk to the
    // upstream iperf3 server and return a complete JSON summary containing
    // traffic. This covers the opposite client/server direction. The scenario
    // override keeps metrics from this reference-server run separate from the
    // iperf3-rs-to-iperf3-rs client metrics asserted below.
    let iperf3rs_to_upstream_output =
        retry_json_client_output("iperf3-rs client to upstream server", || {
            project.client_output(&[
                "iperf3-rs",
                "--push.label",
                "scenario=tcp-reference",
                "-c",
                "reference",
                "-t",
                "1",
                "-J",
            ])
        });
    assert_stdout_is_json_document(&iperf3rs_to_upstream_output);
    let iperf3rs_to_upstream =
        parse_iperf_summary(&iperf3rs_to_upstream_output).expect("iperf JSON should parse");
    assert_iperf_summary_has_traffic(&iperf3rs_to_upstream);

    // Metrics check: run iperf3-rs against iperf3-rs. Pushgateway
    // configuration comes from the `client-rs` service environment, so this is
    // deliberately just a normal iperf client command.
    let iperf3rs_to_iperf3rs_output = project.client_output(&[
        "iperf3-rs",
        "--push.label",
        "scenario=tcp",
        "-c",
        "server-rs",
        "-t",
        "3",
        "-i",
        "1",
    ]);
    assert_success(
        &["iperf3-rs metrics-enabled human stdout"],
        &iperf3rs_to_iperf3rs_output,
    );
    assert_human_iperf_output(&iperf3rs_to_iperf3rs_output);

    // Scrape Pushgateway and require non-zero traffic plus TCP_INFO samples.
    // The metric names prove that the pusher emitted the expected metric
    // families; the label filters prove they came from this integration
    // scenario rather than from another stale Pushgateway group.
    wait_for_pushgateway_metrics(
        &project,
        CLIENT_SCENARIO,
        &[
            "iperf3_bytes",
            "iperf3_bandwidth",
            "iperf3_tcp_rtt_seconds",
            "iperf3_tcp_rttvar_seconds",
            "iperf3_tcp_snd_cwnd_bytes",
            "iperf3_tcp_snd_wnd_bytes",
            "iperf3_tcp_pmtu_bytes",
        ],
    );
    assert_metric_absent(&project, "iperf3_udp_packets", CLIENT_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_lost_packets", CLIENT_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_jitter_seconds", CLIENT_SCENARIO);
    assert_metric_absent(&project, "iperf3_udp_out_of_order_packets", CLIENT_SCENARIO);
    assert_metric_value_eq(&project, "iperf3_omitted", CLIENT_SCENARIO, 0.0);

    // The same long-running server should keep its callback and Pushgateway
    // configuration across client connections. Pushgateway maintains
    // `push_time_seconds` per grouping key, so requiring it to increase after
    // the second connection catches regressions where server callbacks only
    // work for the first accepted test.
    wait_for_metric_value_gt(
        &project,
        "push_time_seconds",
        SERVER_SCENARIO,
        first_server_push,
    );

    // Live push check: the client process runs long enough that metrics should
    // be visible before the iperf command exits. This directly protects the
    // interval-push behavior rather than only observing the final retained
    // Pushgateway sample after process completion.
    let live_args = [
        "iperf3-rs",
        "--push.label",
        "scenario=tcp-live",
        "-c",
        "server-rs",
        "-t",
        "6",
        "-i",
        "1",
    ];
    let mut live_client = project.spawn_client(&live_args);
    wait_for_pushgateway_metrics(
        &project,
        LIVE_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );
    assert!(
        live_client
            .try_wait()
            .expect("failed to poll live client")
            .is_none(),
        "live client exited before interval metrics were observed"
    );
    assert_child_success(&live_args, live_client);

    // File metrics check: use the same Compose service that has Pushgateway
    // defaults to prove the callback fan-out can write a Prometheus metrics file
    // and still leave normal iperf stdout untouched.
    let file_output = project.client_output(&[
        "sh",
        "-eu",
        "-c",
        r#"
metrics=/tmp/iperf3-rs-file.prom
stdout=/tmp/iperf3-rs-file.out
iperf3-rs \
  --push.label scenario=tcp-file \
  --metrics.file "$metrics" \
  --metrics.format prometheus \
  -c server-rs -t 3 -i 1 > "$stdout"
test -s "$metrics"
awk '$1 == "iperf3_bytes" && $2 > 0 { found=1 } END { exit found ? 0 : 1 }' "$metrics" || {
  echo "missing positive iperf3_bytes in metrics file" >&2
  cat "$metrics" >&2
  exit 1
}
awk '$1 == "iperf3_bandwidth" && $2 > 0 { found=1 } END { exit found ? 0 : 1 }' "$metrics" || {
  echo "missing positive iperf3_bandwidth in metrics file" >&2
  cat "$metrics" >&2
  exit 1
}
! grep -q '"event":"interval"' "$metrics"
cat "$stdout"
"#,
    ]);
    assert_success(
        &["iperf3-rs Prometheus metrics file and Pushgateway fan-out"],
        &file_output,
    );
    assert_human_iperf_output(&file_output);
    wait_for_pushgateway_metrics(
        &project,
        FILE_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );

    // Delete-on-exit check: require retained metrics to appear while the client
    // is still running, then require the same grouping key to disappear after
    // the process exits and sends DELETE to Pushgateway.
    let delete_args = [
        "iperf3-rs",
        "--push.label",
        "scenario=tcp-delete",
        "--push.delete-on-exit",
        "-c",
        "server-rs",
        "-t",
        "6",
        "-i",
        "1",
    ];
    let mut delete_client = project.spawn_client(&delete_args);
    wait_for_pushgateway_metrics(
        &project,
        DELETE_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );
    assert!(
        delete_client
            .try_wait()
            .expect("failed to poll delete-on-exit client")
            .is_none(),
        "delete-on-exit client exited before live metrics were observed"
    );
    assert_child_success(&delete_args, delete_client);
    wait_for_metric_absent(&project, "iperf3_bytes", DELETE_SCENARIO);
    assert_metric_absent(&project, "iperf3_bandwidth", DELETE_SCENARIO);

    // Window push check: `--push.interval` intentionally changes the exported
    // metric family names. The Pushgateway should retain representative window
    // summaries, and the same grouping key should not also receive immediate
    // interval gauges that would make the metric semantics ambiguous.
    let window_output = project.client_output(&[
        "iperf3-rs",
        "--push.label",
        "scenario=tcp-window",
        "--push.interval",
        "2s",
        "-c",
        "server-rs",
        "-t",
        "3",
        "-i",
        "1",
    ]);
    assert_success(
        &["iperf3-rs window metrics-enabled human stdout"],
        &window_output,
    );
    assert_human_iperf_output(&window_output);
    wait_for_pushgateway_metrics(
        &project,
        WINDOW_SCENARIO,
        &[
            "iperf3_window_duration_seconds",
            "iperf3_window_transferred_bytes",
            "iperf3_window_bandwidth_mean_bytes_per_second",
            "iperf3_window_tcp_rtt_mean_seconds",
        ],
    );
    assert_metric_absent(&project, "iperf3_bandwidth", WINDOW_SCENARIO);

    // UDP uses different interval fields from TCP. Requiring UDP-prefixed packet
    // metrics in addition to bytes and bandwidth ensures the sender-side UDP
    // mapping is exercised by the Docker integration path.
    let udp = retry_json_client("iperf3-rs UDP client to iperf3-rs server", || {
        project.client_output(&[
            "iperf3-rs",
            "--push.label",
            "scenario=udp",
            "-c",
            "server-rs",
            "-u",
            "-b",
            "1M",
            "-t",
            "3",
            "-i",
            "1",
            "-J",
        ])
    });
    assert_iperf_summary_has_traffic(&udp);
    wait_for_pushgateway_metrics(
        &project,
        UDP_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth", "iperf3_udp_packets"],
    );
    assert_metric_value_eq(
        &project,
        "iperf3_udp_out_of_order_packets",
        UDP_SCENARIO,
        0.0,
    );
    // Jitter is measured by the UDP receiver, so it should appear on the
    // metrics-enabled server rather than on the sending client.
    wait_for_metric_value_gt(&project, "iperf3_udp_jitter_seconds", SERVER_SCENARIO, 0.0);
    assert_metric_value_eq(
        &project,
        "iperf3_udp_out_of_order_packets",
        SERVER_SCENARIO,
        0.0,
    );

    // Reverse mode flips the traffic direction while retaining the client
    // control path. It is a common iperf3 workflow and exercises a different
    // JSON shape from a plain sender-side TCP run.
    let reverse = retry_json_client("iperf3-rs reverse TCP client", || {
        project.client_output(&[
            "iperf3-rs",
            "--push.label",
            "scenario=tcp-reverse",
            "-c",
            "server-rs",
            "-R",
            "-t",
            "3",
            "-i",
            "1",
            "-J",
        ])
    });
    assert_iperf_summary_has_traffic(&reverse);
    wait_for_pushgateway_metrics(
        &project,
        REVERSE_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );

    // Bidirectional mode emits both forward and reverse stream data. Requiring
    // Pushgateway samples for it protects the interval aggregation path from
    // dropping bidirectional runs entirely.
    let bidir = retry_json_client("iperf3-rs bidirectional TCP client", || {
        project.client_output(&[
            "iperf3-rs",
            "--push.label",
            "scenario=tcp-bidir",
            "-c",
            "server-rs",
            "--bidir",
            "-t",
            "3",
            "-i",
            "1",
            "-J",
        ])
    });
    assert_iperf_summary_has_traffic(&bidir);
    wait_for_pushgateway_metrics(
        &project,
        BIDIR_SCENARIO,
        &["iperf3_bytes", "iperf3_bandwidth"],
    );
}

// This smoke test protects the Docker image shape used by release publishing.
//
// The Compose integration test above runs in the `integration` target,
// which intentionally includes test tools such as curl and upstream iperf3.
// The image published to GHCR is the `release` target instead: a scratch image
// that contains only the iperf3-rs binary and the minimal writable filesystem
// libiperf needs at runtime. Building that target, running `--version`, and
// completing a release-image-to-release-image iperf run verifies that the final
// image can start without a shell or dynamic loader and still create real test
// streams. Protocol interoperability with upstream iperf3 and Pushgateway
// behavior remain covered by the Compose test above.
#[test]
#[ignore = "requires Docker"]
fn release_image_smoke() {
    let image = ReleaseImage::build();
    let output = Command::new(&image.docker)
        .arg("run")
        .arg("--rm")
        .arg(&image.tag)
        .arg("--version")
        .output()
        .expect("failed to run release image");

    assert_command_success(&format!("docker run --rm {} --version", image.tag), &output);

    let version_output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        version_output.to_ascii_lowercase().contains("iperf"),
        "release image --version output should identify iperf\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let network = DockerNetwork::create(&image.docker);
    let server = DockerContainer::run_detached(&image.docker, &image.tag, &network.name, &["-s"]);
    let release_to_release =
        retry_json_client("release image client to release image server", || {
            Command::new(&image.docker)
                .arg("run")
                .arg("--rm")
                .arg("--network")
                .arg(&network.name)
                .arg(&image.tag)
                .arg("-c")
                .arg(&server.name)
                .arg("-t")
                .arg("1")
                .arg("-i")
                .arg("1")
                .arg("-J")
                .output()
                .expect("failed to run release image client")
        });
    assert_iperf_summary_has_traffic(&release_to_release);
}

struct ComposeProject {
    command: Vec<OsString>,
    compose_file: PathBuf,
    project_name: String,
}

impl ComposeProject {
    fn new() -> Self {
        // Use a unique Compose project name so the Drop cleanup removes only
        // this test's containers, network, and volumes.
        let project_name = format!("iperf3rsit{}", unique_suffix());

        Self {
            command: compose_command(),
            compose_file: repo_root().join(COMPOSE_FILE),
            project_name,
        }
    }

    fn run_compose(&self, args: &[&str]) {
        let status = self
            .base_command(args)
            .status()
            .expect("failed to run docker compose");
        assert!(status.success(), "docker compose failed with {status}");
    }

    fn spawn_client(&self, args: &[&str]) -> Child {
        let mut compose_args = vec!["run", "--rm", "client-rs"];
        compose_args.extend_from_slice(args);
        self.base_command(&compose_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start docker compose client")
    }

    fn client_output(&self, args: &[&str]) -> Output {
        let mut compose_args = vec!["run", "--rm", "client-rs"];
        compose_args.extend_from_slice(args);
        self.output(&compose_args)
    }

    fn output(&self, args: &[&str]) -> Output {
        self.base_command(args)
            .output()
            .expect("failed to run docker compose")
    }

    fn service_logs(&self, service: &str) -> Output {
        self.output(&["logs", "--no-color", service])
    }

    fn base_command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.command[0]);
        command.args(&self.command[1..]);
        command
            .arg("-p")
            .arg(&self.project_name)
            .arg("-f")
            .arg(&self.compose_file)
            .args(args);
        command
    }
}

impl Drop for ComposeProject {
    fn drop(&mut self) {
        // Best-effort cleanup keeps failure output intact while still removing
        // containers and networks created for this test project.
        let _ = self
            .base_command(&["down", "--volumes", "--remove-orphans"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

struct ReleaseImage {
    docker: OsString,
    tag: String,
    remove_on_drop: bool,
}

impl ReleaseImage {
    fn build() -> Self {
        let docker = docker_command();
        if let Some(tag) = nonempty_env("RELEASE_SMOKE_IMAGE") {
            return Self {
                docker,
                tag,
                remove_on_drop: false,
            };
        }

        let tag = format!("iperf3-rs:release-smoke-{}", unique_suffix());
        let output = Command::new(&docker)
            .arg("build")
            .arg("--target")
            .arg("release")
            .arg("-t")
            .arg(&tag)
            .arg(".")
            .current_dir(repo_root())
            .output()
            .expect("failed to build release image");

        assert_command_success(
            &format!("docker build --target release -t {tag} ."),
            &output,
        );

        Self {
            docker,
            tag,
            remove_on_drop: true,
        }
    }
}

impl Drop for ReleaseImage {
    fn drop(&mut self) {
        if !self.remove_on_drop {
            return;
        }

        // Best-effort cleanup keeps the failure output from the test command
        // visible while removing the unique local image tag created for the
        // smoke test.
        let _ = Command::new(&self.docker)
            .arg("rmi")
            .arg("-f")
            .arg(&self.tag)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

struct DockerNetwork {
    docker: OsString,
    name: String,
}

impl DockerNetwork {
    fn create(docker: &OsString) -> Self {
        let name = format!("iperf3rs-release-smoke-{}", unique_suffix());
        let output = Command::new(docker)
            .arg("network")
            .arg("create")
            .arg(&name)
            .output()
            .expect("failed to create Docker network");

        assert_command_success(&format!("docker network create {name}"), &output);

        Self {
            docker: docker.clone(),
            name,
        }
    }
}

impl Drop for DockerNetwork {
    fn drop(&mut self) {
        let _ = Command::new(&self.docker)
            .arg("network")
            .arg("rm")
            .arg(&self.name)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

struct DockerContainer {
    docker: OsString,
    name: String,
}

impl DockerContainer {
    fn run_detached(docker: &OsString, image: &str, network: &str, args: &[&str]) -> Self {
        let name = format!("iperf3rs-release-smoke-server-{}", unique_suffix());
        let output = Command::new(docker)
            .arg("run")
            .arg("-d")
            .arg("--name")
            .arg(&name)
            .arg("--network")
            .arg(network)
            .arg(image)
            .args(args)
            .output()
            .expect("failed to start Docker container");

        assert_command_success(
            &format!("docker run -d --name {name} {image} {args:?}"),
            &output,
        );

        Self {
            docker: docker.clone(),
            name,
        }
    }
}

impl Drop for DockerContainer {
    fn drop(&mut self) {
        let _ = Command::new(&self.docker)
            .arg("rm")
            .arg("-f")
            .arg(&self.name)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn compose_command() -> Vec<OsString> {
    // Allow Makefile-driven tests to pass either `docker compose` or a
    // `docker-compose` binary path through COMPOSE. Falling back to Docker
    // Compose v2 keeps direct `cargo test --test integration_test -- --ignored`
    // usable on a normal Docker installation.
    env::var_os("COMPOSE")
        .and_then(|raw| {
            let parts = raw
                .to_string_lossy()
                .split_whitespace()
                .map(OsString::from)
                .collect::<Vec<_>>();
            (!parts.is_empty()).then_some(parts)
        })
        .unwrap_or_else(|| vec![OsString::from("docker"), OsString::from("compose")])
}

fn docker_command() -> OsString {
    env::var_os("DOCKER").unwrap_or_else(|| OsString::from("docker"))
}

fn truthy_env(key: &str) -> bool {
    nonempty_env(key)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or_default()
}

fn nonempty_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn unique_suffix() -> String {
    format!(
        "{}{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    )
}

fn retry_json_client(label: &str, run: impl FnMut() -> Output) -> Value {
    let output = retry_json_client_output(label, run);
    parse_iperf_summary(&output).expect("iperf JSON should be valid after successful retry")
}

fn retry_json_client_output(label: &str, mut run: impl FnMut() -> Output) -> Output {
    // iperf servers can take a moment to accept connections after Compose marks
    // the containers as started. Retrying here makes startup ordering explicit
    // without weakening the final success condition: the last successful output
    // still has to be parseable iperf JSON, and callers assert traffic.
    wait_for(label, || {
        let output = run();
        if !output.status.success() || parse_iperf_summary(&output).is_none() {
            return output;
        }
        output
    })
}

fn wait_for(label: &str, mut run: impl FnMut() -> Output) -> Output {
    // Poll command-style checks until they return success, and include the last
    // stdout/stderr in the panic so failures are actionable in CI logs.
    let mut last = None;
    for _ in 0..30 {
        let output = run();
        if output.status.success() {
            return output;
        }

        last = Some(output);
        thread::sleep(Duration::from_secs(1));
    }

    let output = last.expect("wait loop should run at least once");
    panic!(
        "{label} did not succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn parse_iperf_summary(output: &Output) -> Option<Value> {
    // A valid interop result must be a complete iperf JSON document. Requiring
    // both `start` and `end` avoids accepting partial output, and requiring
    // non-zero bytes proves that a real test stream ran.
    let json: Value = serde_json::from_slice(&output.stdout)
        .ok()
        .or_else(|| parse_iperf_json_stream_summary(&output.stdout))?;
    complete_iperf_summary(json)
}

fn complete_iperf_summary(json: Value) -> Option<Value> {
    let has_start = json.get("start").is_some();
    let has_end = json.get("end").is_some();
    (has_start && has_end && iperf_summary_bytes(&json) > 0.0).then_some(json)
}

fn parse_iperf_json_stream_summary(raw: &[u8]) -> Option<Value> {
    // Accept JSON stream output when the caller explicitly requests upstream
    // `--json-stream`; plain `-J` should still be a single complete JSON
    // document even when Pushgateway metrics are enabled.
    let text = std::str::from_utf8(raw).ok()?;
    let mut start = None;
    let mut end = None;

    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match event.get("event").and_then(Value::as_str) {
            Some("start") => start = event.get("data").cloned(),
            Some("end") => end = event.get("data").cloned(),
            _ => {}
        }
    }

    Some(json!({
        "start": start?,
        "end": end?,
    }))
}

fn assert_stdout_is_json_document(output: &Output) {
    let json = serde_json::from_slice::<Value>(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout should be a complete JSON document, got {err}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert!(
        complete_iperf_summary(json).is_some(),
        "stdout JSON should be a complete iperf summary\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

fn assert_human_iperf_output(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[ ID]") && stdout.contains("Interval") && stdout.contains("sender"),
        "metrics-enabled iperf3-rs should preserve human-readable iperf stdout\nstdout:\n{}\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        serde_json::from_slice::<Value>(&output.stdout).is_err(),
        "metrics-enabled iperf3-rs without -J should not replace human stdout with JSON"
    );
}

fn wait_for_service_log_contains(project: &ComposeProject, service: &str, needle: &str) {
    wait_for(&format!("{service} logs contain {needle:?}"), || {
        let output = project.service_logs(service);
        if !output.status.success() {
            return output;
        }

        let logs = String::from_utf8_lossy(&output.stdout);
        if logs.contains(needle) {
            output
        } else {
            failed_output_like(output)
        }
    });
}

fn assert_iperf_summary_has_traffic(json: &Value) {
    assert!(
        iperf_summary_bytes(json) > 0.0,
        "iperf JSON summary should contain non-zero bytes: {json}"
    );
}

fn iperf_summary_bytes(json: &Value) -> f64 {
    [
        &["end", "sum_received", "bytes"][..],
        &["end", "sum_sent", "bytes"][..],
        &["end", "sum", "bytes"][..],
    ]
    .into_iter()
    .find_map(|path| json_path_number(json, path))
    .unwrap_or_default()
}

fn json_path_number(json: &Value, path: &[&str]) -> Option<f64> {
    let mut value = json;
    for segment in path {
        value = value.get(*segment)?;
    }
    value.as_f64().or_else(|| value.as_u64().map(|v| v as f64))
}

fn wait_for_pushgateway_metrics(
    project: &ComposeProject,
    scenario: &str,
    required_metrics: &[&str],
) {
    wait_for(&format!("pushgateway metrics for {scenario}"), || {
        let output =
            project.client_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/metrics")]);
        if !output.status.success() {
            return output;
        }

        let metrics = String::from_utf8_lossy(&output.stdout);
        if required_metrics
            .iter()
            .all(|name| metric_value_gt_zero(&metrics, name, scenario))
        {
            output
        } else {
            failed_output_like(output)
        }
    });
}

fn wait_for_metric_value_gt(project: &ComposeProject, name: &str, scenario: &str, min: f64) -> f64 {
    let output = wait_for(
        &format!("pushgateway {name} for {scenario} > {min}"),
        || {
            let output =
                project.client_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/metrics")]);
            if !output.status.success() {
                return output;
            }

            let metrics = String::from_utf8_lossy(&output.stdout);
            match metric_value(&metrics, name, scenario) {
                Some(value) if value > min => output,
                _ => failed_output_like(output),
            }
        },
    );
    let metrics = String::from_utf8_lossy(&output.stdout);
    metric_value(&metrics, name, scenario).expect("metric value should exist after successful wait")
}

fn assert_metric_value_eq(project: &ComposeProject, name: &str, scenario: &str, expected: f64) {
    let output = project.client_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/metrics")]);
    assert_command_success("pushgateway metrics scrape", &output);
    let metrics = String::from_utf8_lossy(&output.stdout);
    let actual = metric_value(&metrics, name, scenario)
        .unwrap_or_else(|| panic!("missing metric {name} for {scenario}\nmetrics:\n{metrics}"));
    assert_eq!(
        actual, expected,
        "unexpected metric {name} for {scenario}\nmetrics:\n{metrics}"
    );
}

fn assert_metric_absent(project: &ComposeProject, name: &str, scenario: &str) {
    let output = project.client_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/metrics")]);
    assert_command_success("pushgateway metrics scrape", &output);
    let metrics = String::from_utf8_lossy(&output.stdout);
    assert!(
        metric_value(&metrics, name, scenario).is_none(),
        "metric {name} should be absent for {scenario}\nmetrics:\n{metrics}"
    );
}

fn wait_for_metric_absent(project: &ComposeProject, name: &str, scenario: &str) {
    wait_for(
        &format!("pushgateway {name} for {scenario} to be absent"),
        || {
            let output =
                project.client_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/metrics")]);
            if !output.status.success() {
                return output;
            }

            let metrics = String::from_utf8_lossy(&output.stdout);
            if metric_value(&metrics, name, scenario).is_none() {
                output
            } else {
                failed_output_like(output)
            }
        },
    );
}

fn failed_output_like(output: Output) -> Output {
    let mut failed = Command::new("false")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to synthesize failed output");
    failed.stdout = output.stdout;
    failed.stderr = output.stderr;
    failed
}

fn metric_value_gt_zero(metrics: &str, name: &str, scenario: &str) -> bool {
    metric_value(metrics, name, scenario)
        .map(|value| value > 0.0)
        .unwrap_or_default()
}

fn metric_value(metrics: &str, name: &str, scenario: &str) -> Option<f64> {
    // Pushgateway exposes all retained groups on /metrics, so the labels are
    // part of the success condition. `job` is the Pushgateway path root, while
    // `test` and `scenario` are the user-provided grouping labels used by this
    // integration run.
    let prefix = format!("{name}{{");
    let job_label = format!(r#"job="{PUSH_JOB}""#);
    let test_label = format!(r#"test="{PUSH_TEST}""#);
    let scenario_label = format!(r#"scenario="{scenario}""#);
    metrics
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .filter(|line| line.contains(&job_label))
        .filter(|line| line.contains(&test_label))
        .filter(|line| line.contains(&scenario_label))
        .filter_map(|line| line.split_whitespace().last())
        .filter_map(|value| value.parse::<f64>().ok())
        .next()
}

fn assert_success(args: &[&str], output: &Output) {
    assert_command_success(&format!("integration command {args:?}"), output);
}

fn assert_command_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_child_success(args: &[&str], child: Child) {
    let output = child
        .wait_with_output()
        .expect("failed to wait for integration command");
    assert_success(args, &output);
}
