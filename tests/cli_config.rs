#![cfg(all(feature = "pushgateway", feature = "serde"))]

use std::process::Command;

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_iperf3-rs"));
    command.env_clear();
    command
}

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
        let output = cli()
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

#[cfg(unix)]
#[test]
fn option_shaped_operands_reach_upstream_without_being_consumed() {
    for args in [
        vec!["--extra-data", "--push.timeout=bad"],
        vec!["--extra-data", "--help"],
        vec!["--extr", "--version"],
        vec!["-VT--help"],
        vec!["--push.url=localhost:9091", "--push.user-agent", "--help"],
    ] {
        let output = cli()
            .args(["-c", "127.0.0.1", "-p", "0"])
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("failed to parse iperf options")
        );
        assert!(
            !String::from_utf8(output.stdout)
                .unwrap()
                .contains("iperf3-rs options")
        );
    }
    let output = cli().args(["--unknown-option", "--help"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("iperf3-rs options")
    );
    for flag in ["-hV", "--hel", "-Vv"] {
        let output = cli()
            .env("IPERF3_PUSH_TIMEOUT", "bad")
            .arg(flag)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
    }
}

#[cfg(unix)]
mod encoding {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt, process::Output};

    use super::cli;

    fn assert_encoding_error(output: Output, input: &str) {
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(input) && stderr.contains("must be valid UTF-8"));
        assert!(!stderr.contains("panicked"));
    }

    #[test]
    fn non_utf8_argv_is_an_option_error_not_a_panic() {
        assert_encoding_error(
            cli().arg(OsStr::from_bytes(b"\xff")).output().unwrap(),
            "argument 1",
        );
    }

    #[test]
    fn non_utf8_effective_environment_fails_but_overridden_scalars_do_not() {
        for (key, replacement) in [
            ("IPERF3_PUSH_URL", "--push.url=localhost:9091"),
            ("IPERF3_PUSH_JOB", "--push.job=測定"),
            ("IPERF3_PUSH_TIMEOUT", "--push.timeout=5s"),
            ("IPERF3_PUSH_INTERVAL", "--push.interval=1s"),
            ("IPERF3_PUSH_RETRIES", "--push.retries=0"),
            ("IPERF3_PUSH_USER_AGENT", "--push.user-agent=agent"),
            ("IPERF3_PUSH_DELETE_ON_EXIT", "--push.delete-on-exit=false"),
            ("IPERF3_METRICS_FILE", "--metrics.file=測定.jsonl"),
            ("IPERF3_METRICS_PREFIX", "--metrics.prefix=iperf3"),
            ("IPERF3_METRICS_FORMAT", "--metrics.format=jsonl"),
            ("IPERF3_PUSH_LABELS", "--push.label=site=東京"),
            ("IPERF3_METRICS_LABELS", "--metrics.label=site=東京"),
        ] {
            let invalid = OsStr::from_bytes(b"\xff");
            assert_encoding_error(
                cli()
                    .env(key, invalid)
                    .args(["-c", "127.0.0.1", "-p", "0"])
                    .output()
                    .unwrap(),
                key,
            );
            let output = cli()
                .env(key, invalid)
                .args([
                    "-c",
                    "127.0.0.1",
                    "-p",
                    "0",
                    "--push.url=localhost:9091",
                    "--metrics.file=unused.jsonl",
                ])
                .arg(replacement)
                .output()
                .unwrap();
            if key.ends_with("LABELS") {
                // Labels are additive, not overridden: the invalid default is effective.
                assert_encoding_error(output, key);
            } else {
                assert_eq!(output.status.code(), Some(2), "{key}");
                assert!(
                    String::from_utf8(output.stderr)
                        .unwrap()
                        .contains("failed to parse iperf options")
                );
            }
        }
    }

    #[test]
    fn genuine_information_requests_ignore_non_utf8_environment() {
        for flag in ["--help", "--version"] {
            let output = cli()
                .env("IPERF3_METRICS_FILE", OsStr::from_bytes(b"\xff"))
                .env("IPERF3_PUSH_LABELS", OsStr::from_bytes(b"\xff"))
                .arg(flag)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert!(output.stderr.is_empty());
        }
    }
}
