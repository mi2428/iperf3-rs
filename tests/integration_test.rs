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
const SERVER_SCENARIO: &str = "tcp-server";
const LIVE_SCENARIO: &str = "tcp-live";
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
//   server-mode integration labels;
// - an iperf3-rs client can complete a JSON test against upstream iperf3;
// - an iperf3-rs client run from the metrics-enabled `client-rs` service
//   publishes non-zero `iperf3_bytes` and `iperf3_bandwidth` samples with the
//   expected client-mode integration labels;
// - client metrics appear in Pushgateway while a longer run is still active;
// - the long-running iperf3-rs server continues pushing metrics after a later
//   client connection;
// - UDP, reverse TCP, and bidirectional TCP runs all complete and publish
//   client-mode metrics through the same environment-based Pushgateway path.
#[test]
#[ignore = "requires Docker"]
fn compose_interop_and_pushgateway_metrics() {
    let project = ComposeProject::new();

    // Build the shared test image and start the long-running services. The
    // `ComposeProject` uses a unique project name so concurrent or stale test
    // runs do not reuse containers from another invocation.
    project.run_compose(&["build", "client-rs"]);
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
        "server",
        &["iperf3_bytes", "iperf3_bandwidth"],
    );
    let first_server_push = wait_for_metric_value_gt(
        &project,
        "push_time_seconds",
        SERVER_SCENARIO,
        "server",
        0.0,
    );

    // Interop check 2: the iperf3-rs client must be able to talk to the
    // upstream iperf3 server and return a complete JSON summary containing
    // traffic. This covers the opposite client/server direction. The scenario
    // override keeps metrics from this reference-server run separate from the
    // iperf3-rs-to-iperf3-rs client metrics asserted below.
    let iperf3rs_to_upstream = retry_json_client("iperf3-rs client to upstream server", || {
        project.client_output(&[
            "iperf3-rs",
            "--scenario",
            "tcp-reference",
            "-c",
            "reference",
            "-t",
            "1",
            "-J",
        ])
    });
    assert_iperf_summary_has_traffic(&iperf3rs_to_upstream);

    // Metrics check: run iperf3-rs against iperf3-rs. Pushgateway
    // configuration comes from the `client-rs` service environment, so this is
    // deliberately just a normal iperf client command.
    project.run_client(&["iperf3-rs", "-c", "server-rs", "-t", "3", "-i", "1"]);

    // Scrape Pushgateway and require non-zero traffic and bandwidth samples.
    // The metric names prove that the pusher emitted the expected metric
    // families; the label filters prove they came from this integration
    // scenario rather than from another stale Pushgateway group.
    wait_for_pushgateway_metrics(
        &project,
        CLIENT_SCENARIO,
        "client",
        &["iperf3_bytes", "iperf3_bandwidth"],
    );

    // The same long-running server should keep its callback and Pushgateway
    // configuration across client connections. Pushgateway maintains
    // `push_time_seconds` per grouping key, so requiring it to increase after
    // the second connection catches regressions where server callbacks only
    // work for the first accepted test.
    wait_for_metric_value_gt(
        &project,
        "push_time_seconds",
        SERVER_SCENARIO,
        "server",
        first_server_push,
    );

    // Live push check: the client process runs long enough that metrics should
    // be visible before the iperf command exits. This directly protects the
    // interval-push behavior rather than only observing the final retained
    // Pushgateway sample after process completion.
    let live_args = [
        "iperf3-rs",
        "--scenario",
        LIVE_SCENARIO,
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
        "client",
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

    // UDP uses different interval fields from TCP. Requiring packets in
    // addition to bytes and bandwidth ensures the UDP-specific metric mapping
    // is exercised by the Docker integration path.
    let udp = retry_json_client("iperf3-rs UDP client to iperf3-rs server", || {
        project.client_output(&[
            "iperf3-rs",
            "--scenario",
            UDP_SCENARIO,
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
        "client",
        &["iperf3_bytes", "iperf3_bandwidth", "iperf3_packets"],
    );

    // Reverse mode flips the traffic direction while retaining the client
    // control path. It is a common iperf3 workflow and exercises a different
    // JSON shape from a plain sender-side TCP run.
    let reverse = retry_json_client("iperf3-rs reverse TCP client", || {
        project.client_output(&[
            "iperf3-rs",
            "--scenario",
            REVERSE_SCENARIO,
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
        "client",
        &["iperf3_bytes", "iperf3_bandwidth"],
    );

    // Bidirectional mode emits both forward and reverse stream data. This
    // protects the summary/fallback parsing used by the metrics reporter for
    // `sum_bidir_reverse`-style interval events.
    let bidir = retry_json_client("iperf3-rs bidirectional TCP client", || {
        project.client_output(&[
            "iperf3-rs",
            "--scenario",
            BIDIR_SCENARIO,
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
        "client",
        &["iperf3_bytes", "iperf3_bandwidth"],
    );
}

struct ComposeProject {
    command: Vec<OsString>,
    compose_file: PathBuf,
    project_name: String,
}

impl ComposeProject {
    fn new() -> Self {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        // Use a unique Compose project name so the Drop cleanup removes only
        // this test's containers, network, and volumes.
        let project_name = format!(
            "iperf3rsit{}{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        );

        Self {
            command: compose_command(),
            compose_file: manifest_dir.join(COMPOSE_FILE),
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

    fn run_client(&self, args: &[&str]) {
        let output = self.client_output(args);
        assert_success(args, &output);
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

fn retry_json_client(label: &str, mut run: impl FnMut() -> Output) -> Value {
    // iperf servers can take a moment to accept connections after Compose marks
    // the containers as started. Retrying here makes startup ordering explicit
    // without weakening the final success condition: the last successful output
    // still has to be valid iperf JSON with non-zero transferred bytes.
    let output = wait_for(label, || {
        let output = run();
        if !output.status.success() || parse_iperf_summary(&output).is_none() {
            return output;
        }
        output
    });

    parse_iperf_summary(&output).expect("iperf JSON should be valid after successful retry")
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
    // `client-rs` has Pushgateway configured in its environment. When that
    // service runs `iperf3-rs -J`, the wrapper enables libiperf's JSON callback
    // internally for metrics and mirrors callback events because the caller
    // requested JSON output. Accepting JSON stream output here keeps the
    // interop assertion focused on the iperf result instead of on the stdout
    // encoding chosen by the metrics path.
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
    mode: &str,
    required_metrics: &[&str],
) {
    wait_for(
        &format!("pushgateway metrics for {scenario}/{mode}"),
        || {
            let output =
                project.client_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/metrics")]);
            if !output.status.success() {
                return output;
            }

            let metrics = String::from_utf8_lossy(&output.stdout);
            if required_metrics
                .iter()
                .all(|name| metric_value_gt_zero(&metrics, name, scenario, mode))
            {
                output
            } else {
                failed_output_like(output)
            }
        },
    );
}

fn wait_for_metric_value_gt(
    project: &ComposeProject,
    name: &str,
    scenario: &str,
    mode: &str,
    min: f64,
) -> f64 {
    let output = wait_for(
        &format!("pushgateway {name} for {scenario}/{mode} > {min}"),
        || {
            let output =
                project.client_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/metrics")]);
            if !output.status.success() {
                return output;
            }

            let metrics = String::from_utf8_lossy(&output.stdout);
            match metric_value(&metrics, name, scenario, mode) {
                Some(value) if value > min => output,
                _ => failed_output_like(output),
            }
        },
    );
    let metrics = String::from_utf8_lossy(&output.stdout);
    metric_value(&metrics, name, scenario, mode)
        .expect("metric value should exist after successful wait")
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

fn metric_value_gt_zero(metrics: &str, name: &str, scenario: &str, mode: &str) -> bool {
    metric_value(metrics, name, scenario, mode)
        .map(|value| value > 0.0)
        .unwrap_or_default()
}

fn metric_value(metrics: &str, name: &str, scenario: &str, mode: &str) -> Option<f64> {
    // Pushgateway exposes all retained groups on /metrics, so the labels are
    // part of the success condition. They scope the assertion to the iperf3-rs
    // run performed above.
    let prefix = format!("{name}{{");
    let job_label = format!(r#"job="{PUSH_JOB}""#);
    let test_label = format!(r#"test="{PUSH_TEST}""#);
    let scenario_label = format!(r#"scenario="{scenario}""#);
    let mode_label = format!(r#"iperf_mode="{mode}""#);
    metrics
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .filter(|line| line.contains(&job_label))
        .filter(|line| line.contains(&test_label))
        .filter(|line| line.contains(&scenario_label))
        .filter(|line| line.contains(&mode_label))
        .filter_map(|line| line.split_whitespace().last())
        .filter_map(|value| value.parse::<f64>().ok())
        .next()
}

fn assert_success(args: &[&str], output: &Output) {
    assert!(
        output.status.success(),
        "integration command failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
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
