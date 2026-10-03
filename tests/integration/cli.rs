use std::{fs, process::Command};

use super::helpers::*;

#[test]
fn cli_rejects_reserved_labels_before_output_or_delivery() {
    let path = temp_metrics_path("prom");
    let path_arg = path.to_str().unwrap();
    for options in [
        vec![
            "--metrics.file",
            path_arg,
            "--metrics.format",
            "prometheus",
            "--metrics.label",
            "__name__=collision",
        ],
        vec![
            "--push.url",
            "127.0.0.1:9091",
            "--push.label",
            "__private=value",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_iperf3-rs"))
            .args(options)
            .args(["-c", "127.0.0.1"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("reserved"));
        assert!(!path.exists());
    }
}

#[test]
fn cli_rejects_unrepresentable_push_deadlines_before_running() {
    for option in ["--push.timeout", "--push.interval"] {
        let output = Command::new(env!("CARGO_BIN_EXE_iperf3-rs"))
            .args([
                "--push.url",
                "127.0.0.1:9091",
                option,
                "18446744073709551615s",
                "-c",
                "127.0.0.1",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("deadline range"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
    }
}

#[test]
fn cli_files_progress_while_immediate_or_window_http_is_held() {
    use std::io::{ErrorKind, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    for window in [false, true] {
        let port = free_loopback_port();
        let _server = OneOffServer::start(port);
        let path = temp_metrics_path("jsonl");
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let http = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut requests = Vec::new();
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let request = read_http_request(&mut stream);
                        if requests.is_empty() {
                            ready_tx.send(()).unwrap();
                            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                        }
                        requests.push(request);
                        stream.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                    }
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {
                        if stop_rx.try_recv().is_ok() {
                            break;
                        }
                        assert!(Instant::now() < deadline);
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(err) => panic!("accept HTTP: {err}"),
                }
            }
            requests
        });
        let client_path = path.clone();
        let client = thread::spawn(move || {
            let mut options = vec![
                "-t",
                "2",
                "-i",
                "0.1",
                "-J",
                "--push.url",
                endpoint.as_str(),
                "--push.timeout",
                "5s",
            ];
            if window {
                options.extend(["--push.interval", "200ms"]);
            }
            run_cli_metrics_file_client(port, &client_path, &options)
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let valid_records = || {
            fs::read_to_string(&path)
                .unwrap()
                .lines()
                .filter(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
                .count()
        };
        let initial = valid_records();
        let deadline = Instant::now() + Duration::from_secs(2);
        let progressed = loop {
            if valid_records() >= initial + 2 {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            thread::sleep(Duration::from_millis(5));
        };
        release_tx.send(()).unwrap();
        let output = client.join().unwrap();
        stop_tx.send(()).unwrap();
        let requests = http.join().unwrap();
        let records = fs::read_to_string(&path).unwrap();
        for line in records.lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
        fs::remove_file(path).unwrap();
        assert!(
            progressed,
            "required file stalled behind HTTP (window={window})"
        );
        assert!(output.status.success());
        let stdout: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(stdout.get("end").is_some());
        assert!(!requests.is_empty());
        for request in requests {
            assert_eq!(request.contains("iperf3_window_transferred_bytes"), window);
        }
    }
}

#[test]
fn cli_required_files_succeed_when_pushgateway_is_unavailable() {
    use std::net::TcpListener;
    for window in [false, true] {
        let port = free_loopback_port();
        let _server = OneOffServer::start(port);
        let path = temp_metrics_path("jsonl");
        let unavailable = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        // Own the negative endpoint until the run ends. It accepts no HTTP
        // requests, so timeout (not another fixture's protocol) makes it fail.
        let address = unavailable.local_addr().unwrap();
        let endpoint = format!("http://{address}");
        assert_eq!(
            TcpListener::bind(address).unwrap_err().kind(),
            std::io::ErrorKind::AddrInUse
        );
        let mut options = vec![
            "-J",
            "--push.url",
            endpoint.as_str(),
            "--push.timeout",
            "50ms",
        ];
        if window {
            options.extend(["--push.interval", "60s"]);
        }
        let output = run_cli_metrics_file_client(port, &path, &options);
        assert_eq!(
            TcpListener::bind(address).unwrap_err().kind(),
            std::io::ErrorKind::AddrInUse
        );
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("failed to push metrics"));
        assert!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout)
                .unwrap()
                .get("end")
                .is_some()
        );
        let contents = fs::read_to_string(&path).unwrap();
        assert!(!contents.is_empty());
        for line in contents.lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
        fs::remove_file(path).unwrap();
    }
}

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
fn cli_preserves_unicode_metrics_paths_and_json_stdout() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);
    let metrics_file = temp_metrics_path("日本語.jsonl");

    let output = run_cli_metrics_file_client(port, &metrics_file, &["-J"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json.get("end").is_some());
    assert!(
        fs::read_to_string(&metrics_file)
            .unwrap()
            .contains(r#""event":"interval""#)
    );
    fs::remove_file(metrics_file).unwrap();
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
#[test]
fn cli_timestamp_formats_are_optional_attached_values() {
    for (extra, has_prefix) in [
        (vec!["--timestamps=PROBE"], true),
        (vec!["--timestamps", "PROBE"], false),
    ] {
        let port = free_loopback_port();
        let _server = OneOffServer::start(port);
        let metrics_file = temp_metrics_path("jsonl");
        let output = run_cli_metrics_file_client(port, &metrics_file, &extra);
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().contains("PROBE"),
            has_prefix
        );
        fs::remove_file(metrics_file).unwrap();
    }
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
#[test]
fn cli_keeps_option_shaped_data_and_title_in_json() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);
    let metrics_file = temp_metrics_path("jsonl");
    let output = run_cli_metrics_file_client(
        port,
        &metrics_file,
        &[
            "-J",
            "--extra-data",
            "--push.timeout=bad",
            "--title",
            "--help",
        ],
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["extra_data"], "--push.timeout=bad");
    assert_eq!(json["title"], "--help");
    fs::remove_file(metrics_file).unwrap();
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
#[test]
fn library_transfers_an_ordinary_file_after_native_preprocessing() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);
    let source = temp_metrics_path("data");
    let bytes = vec![b'x'; 32 * 1024];
    fs::write(&source, &bytes).unwrap();
    let mut command = iperf3_rs::IperfCommand::client("127.0.0.1");
    command
        .port(port)
        .json()
        .args(["-F", source.to_str().unwrap()]);
    let result = command.run();
    fs::remove_file(source).unwrap();
    let result = result.expect("native file transfer should complete after the listen marker");
    let json: serde_json::Value = serde_json::from_str(result.json_output().unwrap()).unwrap();
    assert!(json["end"]["sum_sent"]["bytes"].as_u64().unwrap() >= bytes.len() as u64);
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
