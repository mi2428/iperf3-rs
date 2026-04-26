use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const COMPOSE_FILE: &str = "docker-compose.test.yml";

// This example integration test protects the library-crate usage path rather
// than the iperf3-rs CLI wrapper path. The checker binary imports iperf3-rs as
// a Rust crate, creates `IperfCommand` clients, consumes interval metrics, and
// applies application-specific pass/fail thresholds.
#[test]
#[ignore = "requires Docker"]
fn bwcheck_validates_multiple_udp_endpoints() {
    let project = ComposeProject::new();

    if !truthy_env("SKIP_EXAMPLE_IMAGE_BUILD") {
        project.run_compose(&["build", "checker"]);
    }
    project.run_compose(&["up", "-d", "server-a", "server-b"]);

    let success = wait_for("bwcheck success", || {
        project.checker_output(&[
            "iperf3-rs-bwcheck",
            "--min-bandwidth-bps",
            "100000",
            "--max-loss-percent",
            "10",
            "server-a:5201",
            "server-b:5201",
        ])
    });
    assert_stdout_contains(&success, "PASS endpoint=server-a:5201");
    assert_stdout_contains(&success, "PASS endpoint=server-b:5201");
    assert_stdout_contains(&success, "summary checked=2 failed=0");

    let failure = project.checker_output(&[
        "iperf3-rs-bwcheck",
        "--min-bandwidth-bps",
        "100000000000",
        "--max-loss-percent",
        "10",
        "server-a:5201",
    ]);
    assert!(
        !failure.status.success(),
        "impossible threshold should fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&failure.stdout),
        String::from_utf8_lossy(&failure.stderr)
    );
    assert_stdout_contains(&failure, "FAIL endpoint=server-a:5201");
    assert_stdout_contains(&failure, "summary checked=1 failed=1");
}

struct ComposeProject {
    command: Vec<OsString>,
    compose_file: PathBuf,
    project_name: String,
}

impl ComposeProject {
    fn new() -> Self {
        Self {
            command: compose_command(),
            compose_file: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(COMPOSE_FILE),
            project_name: format!("iperf3rsbwcheck{}", unique_suffix()),
        }
    }

    fn run_compose(&self, args: &[&str]) {
        let status = self
            .base_command(args)
            .status()
            .expect("failed to run docker compose");
        assert!(status.success(), "docker compose failed with {status}");
    }

    fn checker_output(&self, args: &[&str]) -> Output {
        let mut compose_args = vec!["run", "--rm", "checker"];
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

fn truthy_env(key: &str) -> bool {
    env::var(key)
        .ok()
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or_default()
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

fn wait_for(label: &str, mut run: impl FnMut() -> Output) -> Output {
    let mut last = None;
    for _ in 0..10 {
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

fn assert_stdout_contains(output: &Output, needle: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(needle),
        "stdout should contain {needle:?}\nstdout:\n{}\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
}
