use std::{fs, process::Command};

use super::helpers::*;

#[cfg(all(feature = "pushgateway", feature = "serde"))]
#[test]
fn cli_writes_jsonl_metrics_file_without_replacing_stdout() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);
    let metrics_file = temp_metrics_path("jsonl");

    let output = run_cli_metrics_file_client(port, &metrics_file, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[ ID]"));
    assert!(stdout.contains("sender"));

    let metrics = fs::read_to_string(&metrics_file).unwrap();
    assert!(metrics.lines().any(
        |line| line.contains(r#""schema_version":1"#) && line.contains(r#""event":"interval""#)
    ));
    assert!(metrics.contains(r#""bandwidth_bits_per_second":"#));
    let _ = fs::remove_file(metrics_file);
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
#[test]
fn cli_writes_prometheus_metrics_file_with_custom_prefix() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);
    let metrics_file = temp_metrics_path("prom");

    let output = run_cli_metrics_file_client(
        port,
        &metrics_file,
        &[
            "--metrics.format",
            "prometheus",
            "--metrics.prefix",
            "nettest",
            "--metrics.label",
            "site=ci",
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[ ID]"));
    assert!(stdout.contains("sender"));

    let metrics = fs::read_to_string(&metrics_file).unwrap();
    assert!(metrics.contains("nettest_transferred_bytes{site=\"ci\"} "));
    assert!(metrics.contains("nettest_bandwidth_bits_per_second{site=\"ci\"} "));
    assert!(!metrics.contains("iperf3_transferred_bytes "));
    let _ = fs::remove_file(metrics_file);
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
#[test]
fn cli_treats_metrics_file_create_failure_as_fatal() {
    let missing_dir = temp_metrics_path("missing-dir");
    let metrics_file = missing_dir.join("metrics.jsonl");
    let port = free_loopback_port().to_string();
    let metrics_file_arg = metrics_file.to_string_lossy();

    let output = Command::new(env!("CARGO_BIN_EXE_iperf3-rs"))
        .args([
            "-c",
            "127.0.0.1",
            "-p",
            port.as_str(),
            "--metrics.file",
            metrics_file_arg.as_ref(),
        ])
        .output()
        .expect("run iperf3-rs client with unwritable metrics file");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to create metrics file"),
        "stderr should explain metrics file failure:\n{stderr}"
    );
}
