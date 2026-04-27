use std::{thread, time::Duration};

use super::helpers::*;
use iperf3_rs::{MetricDirection, MetricEvent, MetricsMode, PushGatewayConfig, TransportProtocol};

#[cfg(all(feature = "pushgateway", feature = "serde"))]
#[test]
fn command_spawn_streams_interval_metrics_against_one_off_server() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);

    let (result, events) = run_library_client(port, MetricsMode::Interval);
    let json = result
        .json_value()
        .expect("json output should be retained")
        .expect("json output should parse");
    assert!(json.get("end").is_some());

    let samples = events
        .iter()
        .filter_map(|event| match event {
            MetricEvent::Interval(sample) => Some(sample),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(!samples.is_empty(), "client should emit interval samples");
    assert!(samples.iter().any(|sample| sample.transferred_bytes > 0.0));
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
    assert!(
        samples
            .iter()
            .all(|sample| sample.direction == MetricDirection::Sender)
    );
    assert!(samples.iter().all(|sample| sample.stream_count > 0));
    assert!(samples.iter().all(|sample| sample.udp_packets.is_none()));
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
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
            .any(|window| window.bandwidth_bits_per_second.samples > 0)
    );
    assert!(
        windows
            .iter()
            .all(|window| window.role == iperf3_rs::Role::Client)
    );
    assert!(
        windows
            .iter()
            .all(|window| window.direction == MetricDirection::Sender)
    );
    assert!(
        windows
            .iter()
            .all(|window| window.protocol == TransportProtocol::Tcp)
    );
    assert!(windows.iter().all(|window| window.stream_count > 0));
    assert!(windows.iter().all(|window| window.udp_packets.is_none()));
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
#[test]
fn command_run_collects_interval_metrics_in_result() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);

    let result = run_library_client_blocking(port, MetricsMode::Interval);
    let samples = result
        .metrics()
        .iter()
        .filter_map(|event| match event {
            MetricEvent::Interval(sample) => Some(sample),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(!samples.is_empty(), "blocking run should retain samples");
    assert!(samples.iter().any(|sample| sample.transferred_bytes > 0.0));
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
#[test]
fn command_run_collects_window_metrics_in_result() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);

    let result = run_library_client_blocking(port, MetricsMode::Window(Duration::from_secs(2)));
    let windows = result
        .metrics()
        .iter()
        .filter_map(|event| match event {
            MetricEvent::Window(window) => Some(window),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(!windows.is_empty(), "blocking run should retain windows");
    assert!(windows.iter().any(|window| window.transferred_bytes > 0.0));
    assert!(
        windows
            .iter()
            .all(|window| window.direction == MetricDirection::Sender)
    );
    assert!(windows.iter().all(|window| window.stream_count > 0));
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
#[test]
fn command_run_with_pushgateway_pushes_interval_metrics() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);
    let (sink, endpoint) = OneShotHttpSink::start();
    let config = PushGatewayConfig::new(PushGatewayConfig::parse_endpoint(&endpoint).unwrap())
        .label("scenario", "library-direct")
        .timeout(Duration::from_secs(1));

    let mut last_error = String::new();
    for _ in 0..20 {
        match try_run_library_direct_push_client(port, config.clone()) {
            Ok(()) => {
                let request = sink.wait();
                assert!(request.contains("/metrics/job/iperf3/scenario/library-direct"));
                assert!(request.contains("iperf3_transferred_bytes"));
                assert!(request.contains("iperf3_bandwidth_bits_per_second"));
                return;
            }
            Err(err) => {
                last_error = err.to_string();
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    panic!("client should complete and push metrics: {last_error}");
}
