#[cfg(feature = "serde")]
use std::fs;
use std::time::Duration;

#[cfg(feature = "serde")]
use super::helpers::temp_metrics_path;
use iperf3_rs::{
    ErrorKind, IperfCommand, Metrics, MetricsMode, PrometheusEncoder, WindowMetrics,
    libiperf_version, usage_long,
};
#[cfg(feature = "serde")]
use iperf3_rs::{MetricEvent, MetricsFileFormat, MetricsFileSink};

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
fn public_prometheus_encoder_renders_metrics_without_pushgateway() {
    let encoder = PrometheusEncoder::new("nettest").unwrap();

    let mut sample = Metrics::new();
    sample.transferred_bytes = 32.0;
    sample.bandwidth_bits_per_second = 256.0;
    sample.interval_duration_seconds = 1.0;

    let interval = encoder.encode_interval(&sample);
    assert!(interval.contains("nettest_transferred_bytes 32\n"));
    assert!(interval.contains("nettest_bandwidth_bits_per_second 256\n"));

    let mut window_metrics = WindowMetrics::new();
    window_metrics.duration_seconds = 2.0;
    window_metrics.transferred_bytes = 64.0;

    let window = encoder.encode_window(&window_metrics);
    assert!(window.contains("nettest_window_duration_seconds 2\n"));
    assert!(window.contains("nettest_window_transferred_bytes 64\n"));
}

#[cfg(feature = "serde")]
#[test]
fn public_metrics_file_sink_writes_jsonl_events() {
    let metrics_file = temp_metrics_path("jsonl");
    let sink = MetricsFileSink::new(&metrics_file, MetricsFileFormat::Jsonl).unwrap();

    let mut sample = Metrics::new();
    sample.transferred_bytes = 32.0;
    sample.bandwidth_bits_per_second = 256.0;
    sample.interval_duration_seconds = 1.0;

    sink.write_event(&MetricEvent::Interval(sample)).unwrap();

    let metrics = fs::read_to_string(&metrics_file).unwrap();
    assert!(metrics.contains(r#""schema_version":1"#));
    assert!(metrics.contains(r#""event":"interval""#));
    assert!(metrics.contains(r#""transferred_bytes":32.0"#));
    let _ = fs::remove_file(metrics_file);
}
