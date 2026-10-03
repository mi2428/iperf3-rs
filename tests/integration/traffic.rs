use std::{env, net::TcpListener, process::Command, time::Duration};

use super::process::OwnedCommand;
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
                    .inherit_output()
                    .arg("--forceflush")
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
    let mut server = spawn_fixture("server", port);
    // Read this owned process's flushed listen marker. Opening a TCP readiness
    // connection would consume the one-off server before the actual transfer.
    server
        .wait_for_stdout("Server listening on")
        .expect("bounded server readiness");
    let client = spawn_fixture("client", port);
    for (role, child) in [("client", client), ("server", server)] {
        let output = child.wait().expect("bounded library traffic fixture");
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
    let (running, metrics) = command.spawn_with_metrics(MetricsMode::Interval).unwrap();
    let samples = metrics.collect::<Vec<_>>();
    let result = running.wait().expect("library transfer completes");
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
}

fn spawn_fixture(role: &str, port: u16) -> OwnedCommand {
    OwnedCommand::spawn(
        Command::new(env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env(ROLE, role)
            .env(PORT, port.to_string()),
        Duration::from_secs(10),
    )
    .expect("start library traffic fixture")
}
