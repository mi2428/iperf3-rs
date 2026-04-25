use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

const COMPOSE_FILE: &str = "docker-compose.test.yml";
const PUSHGATEWAY_URL: &str = "http://pushgateway:9091";

#[test]
#[ignore = "requires Docker"]
fn compose_interop_and_pushgateway_metrics() {
    let project = ComposeProject::new();

    project.run_compose(&["build", "runner"]);
    project.run_compose(&["up", "-d", "sut", "reference", "pushgateway"]);

    wait_for("pushgateway readiness", || {
        project.integration_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/-/ready")])
    });

    let upstream_to_iperf3rs = retry_json_client("upstream client to iperf3-rs server", || {
        project.integration_output(&["iperf3", "-c", "sut", "-t", "1", "-J"])
    });
    assert_iperf_summary_has_traffic(&upstream_to_iperf3rs);

    let iperf3rs_to_upstream = retry_json_client("iperf3-rs client to upstream server", || {
        project.integration_output(&["iperf3-rs", "-c", "reference", "-t", "1", "-J"])
    });
    assert_iperf_summary_has_traffic(&iperf3rs_to_upstream);

    project.run_integration(&[
        "iperf3-rs",
        "--push-gateway",
        PUSHGATEWAY_URL,
        "--job",
        "integration",
        "--test",
        "self",
        "--scenario",
        "tcp",
        "-c",
        "sut",
        "-t",
        "3",
        "-i",
        "1",
        "--json-stream",
    ]);

    wait_for("pushgateway metrics", || {
        let output =
            project.integration_output(&["curl", "-fsS", &format!("{PUSHGATEWAY_URL}/metrics")]);
        if !output.status.success() {
            return output;
        }

        let metrics = String::from_utf8_lossy(&output.stdout);
        if metric_value_gt_zero(&metrics, "iperf3_bytes")
            && metric_value_gt_zero(&metrics, "iperf3_bandwidth")
        {
            output
        } else {
            let mut failed = Command::new("false")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .expect("failed to synthesize failed output");
            failed.stdout = output.stdout;
            failed.stderr = output.stderr;
            failed
        }
    });
}

struct ComposeProject {
    command: Vec<OsString>,
    compose_file: PathBuf,
    project_name: String,
}

impl ComposeProject {
    fn new() -> Self {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
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

    fn run_integration(&self, args: &[&str]) {
        let output = self.integration_output(args);
        assert_success(args, &output);
    }

    fn integration_output(&self, args: &[&str]) -> Output {
        let mut compose_args = vec!["run", "--rm", "runner"];
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
        let _ = self
            .base_command(&["down", "--volumes", "--remove-orphans"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn compose_command() -> Vec<OsString> {
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
    let json: Value = serde_json::from_slice(&output.stdout).ok()?;
    let has_start = json.get("start").is_some();
    let has_end = json.get("end").is_some();
    (has_start && has_end && iperf_summary_bytes(&json) > 0.0).then_some(json)
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

fn metric_value_gt_zero(metrics: &str, name: &str) -> bool {
    let prefix = format!("{name}{{");
    metrics
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .filter(|line| line.contains(r#"job="integration""#))
        .filter(|line| line.contains(r#"test="self""#))
        .filter(|line| line.contains(r#"scenario="tcp""#))
        .filter(|line| line.contains(r#"iperf_mode="client""#))
        .filter_map(|line| line.split_whitespace().last())
        .filter_map(|value| value.parse::<f64>().ok())
        .any(|value| value > 0.0)
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
