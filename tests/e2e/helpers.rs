#![allow(dead_code)]

use std::env;
use std::ffi::OsString;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

pub(crate) const COMPOSE_FILE: &str = "docker-compose.test.yml";
pub(crate) const PUSHGATEWAY_URL: &str = "http://pushgateway:9091";
pub(crate) const PUSH_JOB: &str = "integration";
pub(crate) const PUSH_TEST: &str = "self";
pub(crate) const CLIENT_SCENARIO: &str = "tcp";
pub(crate) const FILE_SCENARIO: &str = "tcp-file";
pub(crate) const DELETE_SCENARIO: &str = "tcp-delete";
pub(crate) const SERVER_SCENARIO: &str = "tcp-server";
pub(crate) const LIVE_SCENARIO: &str = "tcp-live";
pub(crate) const WINDOW_SCENARIO: &str = "tcp-window";
pub(crate) const UDP_SCENARIO: &str = "udp";
pub(crate) const REVERSE_SCENARIO: &str = "tcp-reverse";
pub(crate) const BIDIR_SCENARIO: &str = "tcp-bidir";

pub(crate) struct ComposeProject {
    command: Vec<OsString>,
    compose_file: PathBuf,
    project_name: String,
}

impl ComposeProject {
    pub(crate) fn new() -> Self {
        // Use a unique Compose project name so the Drop cleanup removes only
        // this test's containers, network, and volumes.
        let project_name = format!("iperf3rsit{}", unique_suffix());

        Self {
            command: compose_command(),
            compose_file: repo_root().join(COMPOSE_FILE),
            project_name,
        }
    }

    pub(crate) fn run_compose(&self, args: &[&str]) {
        let status = self
            .base_command(args)
            .status()
            .expect("failed to run docker compose");
        assert!(status.success(), "docker compose failed with {status}");
    }

    pub(crate) fn spawn_client(&self, args: &[&str]) -> Child {
        let mut compose_args = vec!["run", "--rm", "client-rs"];
        compose_args.extend_from_slice(args);
        self.base_command(&compose_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start docker compose client")
    }

    pub(crate) fn client_output(&self, args: &[&str]) -> Output {
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

pub(crate) struct ReleaseImage {
    pub(crate) docker: OsString,
    pub(crate) tag: String,
    remove_on_drop: bool,
}

impl ReleaseImage {
    pub(crate) fn build() -> Self {
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

pub(crate) struct DockerNetwork {
    pub(crate) docker: OsString,
    pub(crate) name: String,
}

impl DockerNetwork {
    pub(crate) fn create(docker: &OsString) -> Self {
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

pub(crate) struct DockerContainer {
    pub(crate) docker: OsString,
    pub(crate) name: String,
}

impl DockerContainer {
    pub(crate) fn run_detached(
        docker: &OsString,
        image: &str,
        network: &str,
        args: &[&str],
    ) -> Self {
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
    // Compose v2 keeps direct `cargo test --test e2e_test -- --ignored`
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

pub(crate) fn truthy_env(key: &str) -> bool {
    nonempty_env(key)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or_default()
}

pub(crate) fn skip_e2e_image_build() -> bool {
    truthy_env("SKIP_E2E_IMAGE_BUILD") || truthy_env("SKIP_INTEGRATION_IMAGE_BUILD")
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

pub(crate) fn retry_json_client(label: &str, run: impl FnMut() -> Output) -> Value {
    let output = retry_json_client_output(label, run);
    parse_iperf_summary(&output).expect("iperf JSON should be valid after successful retry")
}

// The Docker E2E remains one Rust test for setup cost, so explicit
// markers make the inner scenario progress visible under `--nocapture`.
pub(crate) fn scenario<T>(name: &str, run: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    eprintln!("\n[e2e] START {name}");

    match panic::catch_unwind(AssertUnwindSafe(run)) {
        Ok(value) => {
            eprintln!(
                "[e2e] PASS  {name} ({:.1}s)",
                started.elapsed().as_secs_f64()
            );
            value
        }
        Err(payload) => {
            eprintln!(
                "[e2e] FAIL  {name} ({:.1}s)",
                started.elapsed().as_secs_f64()
            );
            panic::resume_unwind(payload);
        }
    }
}

pub(crate) fn retry_json_client_output(label: &str, mut run: impl FnMut() -> Output) -> Output {
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

pub(crate) fn wait_for(label: &str, mut run: impl FnMut() -> Output) -> Output {
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

pub(crate) fn parse_iperf_summary(output: &Output) -> Option<Value> {
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

pub(crate) fn assert_stdout_is_json_document(output: &Output) {
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

pub(crate) fn assert_human_iperf_output(output: &Output) {
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

pub(crate) fn wait_for_service_log_contains(project: &ComposeProject, service: &str, needle: &str) {
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

pub(crate) fn assert_iperf_summary_has_traffic(json: &Value) {
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

pub(crate) fn wait_for_pushgateway_metrics(
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

pub(crate) fn wait_for_metric_value_gt(
    project: &ComposeProject,
    name: &str,
    scenario: &str,
    min: f64,
) -> f64 {
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

pub(crate) fn assert_metric_value_eq(
    project: &ComposeProject,
    name: &str,
    scenario: &str,
    expected: f64,
) {
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

pub(crate) fn assert_metric_absent(project: &ComposeProject, name: &str, scenario: &str) {
    let output = project.client_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/metrics")]);
    assert_command_success("pushgateway metrics scrape", &output);
    let metrics = String::from_utf8_lossy(&output.stdout);
    assert!(
        metric_value(&metrics, name, scenario).is_none(),
        "metric {name} should be absent for {scenario}\nmetrics:\n{metrics}"
    );
}

pub(crate) fn wait_for_metric_absent(project: &ComposeProject, name: &str, scenario: &str) {
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
    // E2E run.
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

pub(crate) fn assert_success(args: &[&str], output: &Output) {
    assert_command_success(&format!("E2E command {args:?}"), output);
}

pub(crate) fn assert_command_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn assert_child_success(args: &[&str], child: Child) {
    let output = child
        .wait_with_output()
        .expect("failed to wait for E2E command");
    assert_success(args, &output);
}
