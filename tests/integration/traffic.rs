use std::{
    env,
    net::TcpListener,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use iperf3_rs::{IperfCommand, MetricDirection, MetricEvent, MetricsMode, TransportProtocol};

const TEST: &str = "integration::traffic::library_interval_metrics_real_transfer";
const ROLE: &str = "IPERF3_RS_TRAFFIC_TEST_ROLE";
const PORT: &str = "IPERF3_RS_TRAFFIC_TEST_PORT";

// Re-enter this test in dedicated processes: server/client must not share RUN_LOCK.
#[test]
fn library_interval_metrics_real_transfer() {
    if let Ok(role) = env::var(ROLE) {
        let port = env::var(PORT).unwrap().parse().unwrap();
        match role.as_str() {
            "server" => {
                IperfCommand::server_once()
                    .bind("127.0.0.1")
                    .port(port)
                    .run()
                    .expect("one-off library server completes");
            }
            "client" => run_client(port),
            other => panic!("unknown fixture role {other}"),
        }
        return;
    }

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let server = TestProcess::spawn("server", port);
    let client = TestProcess::spawn("client", port);
    for (role, child) in [("client", client), ("server", server)] {
        let output = child.wait(Duration::from_secs(10));
        assert!(
            output.status.success(),
            "{role} fixture failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn run_client(port: u16) {
    let mut command = IperfCommand::client("127.0.0.1");
    command
        .port(port)
        .duration(Duration::from_secs(1))
        .report_interval(Duration::from_secs(1))
        .connect_timeout(Duration::from_secs(1));
    for attempt in 0..20 {
        let (running, metrics) = command.spawn_with_metrics(MetricsMode::Interval).unwrap();
        let samples = metrics.collect::<Vec<_>>();
        match running.wait() {
            Ok(result) => {
                assert!(result.json_output().is_none(), "no JSON/serde needed");
                assert!(!samples.is_empty(), "real transfer must emit intervals");
                assert!(samples.iter().any(|event| matches!(event,
                    MetricEvent::Interval(sample)
                        if sample.transferred_bytes > 0.0
                        && sample.bandwidth_bits_per_second > 0.0
                        && sample.stream_count > 0
                        && sample.protocol == TransportProtocol::Tcp
                        && sample.direction == MetricDirection::Sender
                )));
                return;
            }
            Err(error) if attempt < 19 && error.to_string().contains("Connection refused") => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => panic!("library transfer failed: {error}"),
        }
    }
    unreachable!();
}

struct TestProcess(Option<Child>);

impl TestProcess {
    fn spawn(role: &str, port: u16) -> Self {
        Self(Some(
            Command::new(env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
                .env(ROLE, role)
                .env(PORT, port.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("start library traffic fixture"),
        ))
    }

    fn wait(mut self, timeout: Duration) -> Output {
        let deadline = Instant::now() + timeout;
        while self.0.as_mut().unwrap().try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "traffic fixture exceeded {timeout:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
        // These exact test fixtures only emit the bounded test harness report.
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for TestProcess {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
