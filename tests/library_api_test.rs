use std::{
    env,
    net::TcpListener,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

use iperf3_rs::{
    ErrorKind, IperfCommand, MetricEvent, MetricsMode, TransportProtocol, libiperf_version,
    usage_long,
};

#[test]
fn public_api_exposes_upstream_metadata() {
    assert!(!libiperf_version().trim().is_empty());
    assert!(usage_long().unwrap().contains("Usage:"));
}

#[test]
fn command_rejects_zero_metrics_window() {
    let mut command = IperfCommand::new();
    command.metrics(MetricsMode::Window(Duration::ZERO));

    let err = command.run().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidMetricsMode);
    assert!(err.to_string().contains("greater than zero"), "{err:#}");
}

#[test]
fn command_spawn_streams_interval_metrics_against_one_off_server() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);

    let (_result, events) = run_library_client(port, MetricsMode::Interval);
    let samples = events
        .iter()
        .filter_map(|event| match event {
            MetricEvent::Interval(sample) => Some(sample),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(!samples.is_empty(), "client should emit interval samples");
    assert!(samples.iter().any(|sample| sample.bytes > 0.0));
    assert!(
        samples
            .iter()
            .any(|sample| sample.bandwidth_bits_per_second > 0.0)
    );
    assert!(
        samples
            .iter()
            .all(|sample| sample.protocol == TransportProtocol::Tcp)
    );
    assert!(samples.iter().all(|sample| sample.udp_packets.is_none()));
}

#[test]
fn command_spawn_streams_window_metrics_against_one_off_server() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);

    let (_result, events) = run_library_client(port, MetricsMode::Window(Duration::from_secs(2)));
    let windows = events
        .iter()
        .filter_map(|event| match event {
            MetricEvent::Window(window) => Some(window),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(!windows.is_empty(), "client should emit a final window");
    assert!(windows.iter().any(|window| window.transferred_bytes > 0.0));
    assert!(
        windows
            .iter()
            .any(|window| window.bandwidth_bytes_per_second.samples > 0)
    );
    assert!(windows.iter().all(|window| window.udp_packets.is_none()));
}

fn run_library_client(port: u16, mode: MetricsMode) -> (iperf3_rs::IperfResult, Vec<MetricEvent>) {
    let mut last_error = String::new();
    for _ in 0..20 {
        match try_run_library_client(port, mode) {
            Ok(result) => return result,
            Err(err) => {
                last_error = err.to_string();
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    panic!("client should complete: {last_error}");
}

fn try_run_library_client(
    port: u16,
    mode: MetricsMode,
) -> iperf3_rs::Result<(iperf3_rs::IperfResult, Vec<MetricEvent>)> {
    let logfile = env::temp_dir().join(format!("iperf3-rs-library-api-{port}.log"));
    let logfile = logfile.to_string_lossy();
    let mut command = IperfCommand::client("127.0.0.1");
    command
        .port(port)
        .duration(Duration::from_secs(2))
        .report_interval(Duration::from_secs(1))
        .args(["--logfile", logfile.as_ref()]);

    let (running, mut metrics) = command.spawn_with_metrics(mode)?;
    let events = metrics.by_ref().collect::<Vec<_>>();
    let result = running.wait()?;
    Ok((result, events))
}

fn free_loopback_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral loopback port");
    listener.local_addr().unwrap().port()
}

struct OneOffServer {
    child: Child,
}

impl OneOffServer {
    fn start(port: u16) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_iperf3-rs"))
            .args(["-s", "-1", "-p", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start iperf3-rs one-off server");

        Self { child }
    }
}

impl Drop for OneOffServer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
