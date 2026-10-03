#![cfg(all(feature = "pushgateway", feature = "serde"))]

use std::process::Command;

#[test]
fn cli_scalar_overrides_reach_upstream_validation() {
    for (key, option, valid, invalid) in [
        ("IPERF3_PUSH_TIMEOUT", "--push.timeout", "5s", "bad"),
        ("IPERF3_PUSH_INTERVAL", "--push.interval", "1s", "bad"),
        ("IPERF3_PUSH_RETRIES", "--push.retries", "0", "bad"),
        (
            "IPERF3_PUSH_USER_AGENT",
            "--push.user-agent",
            "agent",
            "bad\nagent",
        ),
        (
            "IPERF3_PUSH_DELETE_ON_EXIT",
            "--push.delete-on-exit",
            "false",
            "bad",
        ),
        (
            "IPERF3_METRICS_PREFIX",
            "--metrics.prefix",
            "iperf3",
            "bad-prefix",
        ),
        ("IPERF3_METRICS_FORMAT", "--metrics.format", "jsonl", "bad"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_iperf3-rs"))
            .env_clear()
            .env(key, invalid)
            .args([
                "-c",
                "127.0.0.1",
                "-p",
                "0",
                "--push.url=localhost:9091",
                "--metrics.file=unused.jsonl",
            ])
            .arg(format!("{option}={valid}"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{key}");
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("failed to parse iperf options")
        );
    }
}
