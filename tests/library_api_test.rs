use std::time::Duration;

use iperf3_rs::{IperfCommand, MetricsMode, libiperf_version, usage_long};

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
    assert!(err.to_string().contains("greater than zero"), "{err:#}");
}
