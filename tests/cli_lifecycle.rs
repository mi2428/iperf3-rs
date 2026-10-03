#![cfg(all(unix, feature = "pushgateway", feature = "serde"))]

use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_iperf3-rs"));
    command.env_clear();
    command
}

fn path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("cli-lifecycle-{}-{name}", std::process::id()))
}

fn port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "test-owned CLI condition timed out"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_child(child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap(); // Only this non-detached Child handle.
            child.wait().unwrap();
            panic!("test-owned foreground/daemon-parent child timed out");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn accept_http(listener: &TcpListener) -> TcpStream {
    let mut connection = None;
    wait_for(|| match listener.accept() {
        Ok((stream, _)) => {
            connection = Some(stream);
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
        Err(error) => panic!("owned HTTP listener failed: {error}"),
    });
    connection.unwrap()
}

fn read_http(connection: &mut TcpStream) -> Vec<u8> {
    connection
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut request = Vec::new();
    let mut length = None;
    loop {
        let mut bytes = [0; 4096];
        let count = connection.read(&mut bytes).unwrap();
        assert!(count > 0);
        request.extend_from_slice(&bytes[..count]);
        if length.is_none()
            && let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
        {
            let headers = std::str::from_utf8(&request[..end]).unwrap();
            let body = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            length = Some(end + 4 + body);
        }
        if length.is_some_and(|length| request.len() >= length) {
            return request;
        }
    }
}

fn release_http(mut connection: TcpStream) {
    connection
        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .unwrap();
}

#[test]
fn cli_pidfiles_are_owned_and_library_runs_do_not_daemonize() {
    let pidfile = path("foreground.pid");
    let mut child = OwnedChild(
        cli()
            .args([
                "-s",
                "-1",
                "-p",
                &port().to_string(),
                "--idle-timeout",
                "1",
                "-I",
            ])
            .arg(&pidfile)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for(|| pidfile.exists());
    wait_child(&mut child.0);
    assert!(!pidfile.exists());

    let missing = path("missing").join("pid");
    let output = cli()
        .args(["-s", "-1", "-I"])
        .arg(&missing)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("failed to prepare CLI runtime")
    );

    fs::write(&pidfile, std::process::id().to_string()).unwrap();
    let output = cli()
        .args(["-s", "-1", "-I"])
        .arg(&pidfile)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(&pidfile).unwrap(),
        std::process::id().to_string()
    );
    fs::remove_file(&pidfile).unwrap();

    let mut server = iperf3_rs::IperfCommand::server_once();
    server
        .port(port())
        .args(["-D", "-I", pidfile.to_str().unwrap(), "--idle-timeout", "1"]);
    assert!(server.run().is_ok());
    assert!(!pidfile.exists());
}

#[test]
fn owned_daemon_finishes_by_idle_timeout_or_normal_client_without_signals() {
    for client in [false, true] {
        let port = port();
        let name = if client { "client" } else { "idle" };
        let pidfile = path(&format!("daemon-{name}.pid"));
        let logfile = path(&format!("daemon-{name}.log"));
        let metrics = path(&format!("daemon-{name}.jsonl"));
        let mut parent = OwnedChild(
            cli()
                .args([
                    "-s",
                    "-1",
                    "-D",
                    "-p",
                    &port.to_string(),
                    "--idle-timeout",
                    "3",
                    "--forceflush",
                    "-I",
                ])
                .arg(&pidfile)
                .arg("--logfile")
                .arg(&logfile)
                .arg("--metrics.file")
                .arg(&metrics)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        wait_child(&mut parent.0);
        let marker = format!("Server listening on {port} ");
        wait_for(|| fs::read_to_string(&logfile).is_ok_and(|text| text.contains(&marker)));
        if client {
            let client_pidfile = path("client.pid");
            let output = cli()
                .args([
                    "-c",
                    "127.0.0.1",
                    "-p",
                    &port.to_string(),
                    "-t",
                    "1",
                    "-b",
                    "1M",
                    "-J",
                ])
                .arg("-I")
                .arg(&client_pidfile)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert!(!client_pidfile.exists());
            let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(json.get("end").is_some());
        }
        wait_for(|| !pidfile.exists());
        if client {
            assert!(!fs::read_to_string(&metrics).unwrap().is_empty());
        }
        fs::remove_file(logfile).unwrap();
        fs::remove_file(metrics).unwrap();
    }
}

#[test]
fn foreground_signal_returns_through_json_and_pidfile_cleanup() {
    let pidfile = path("signal.pid");
    let logfile = path("signal.json");
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let mut child = OwnedChild(
        cli()
            .args([
                "-s",
                "-1",
                "-p",
                &port().to_string(),
                "--idle-timeout",
                "3",
                "-J",
                "-I",
            ])
            .arg(&pidfile)
            .arg("--logfile")
            .arg(&logfile)
            .args([
                "--push.url",
                &endpoint,
                "--push.delete-on-exit",
                "--push.timeout",
                "5s",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for(|| pidfile.exists());
    assert!(child.0.try_wait().unwrap().is_none());
    // This is the direct, unreaped foreground Child, never a pidfile/daemon PID.
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let mut connection = None;
    wait_for(|| match listener.accept() {
        Ok((stream, _)) => {
            connection = Some(stream);
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
        Err(error) => panic!("owned HTTP listener failed: {error}"),
    });
    let mut connection = connection.unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut request = Vec::new();
    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let mut bytes = [0; 512];
        let count = connection.read(&mut bytes).unwrap();
        assert!(count > 0);
        request.extend_from_slice(&bytes[..count]);
    }
    assert!(request.starts_with(b"DELETE "));
    // Holding delivery proves cleanup has not retired its fds/pidfile yet.
    thread::sleep(Duration::from_millis(100));
    assert!(child.0.try_wait().unwrap().is_none());
    assert!(pidfile.exists());
    connection
        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .unwrap();
    drop(connection);
    wait_child(&mut child.0);
    assert!(!pidfile.exists());
    let json: serde_json::Value = serde_json::from_slice(&fs::read(&logfile).unwrap()).unwrap();
    assert!(json["error"].as_str().unwrap().contains("signal"));
    fs::remove_file(logfile).unwrap();
}

#[test]
fn active_foreground_client_signal_stops_before_duration_and_drains_http() {
    let port = port();
    let server_log = path("active-server.log");
    let pidfile = path("active-client.pid");
    let jsonfile = path("active-client.json");
    let metrics = path("active-client.jsonl");
    let mut server = OwnedChild(
        cli()
            .args([
                "-s",
                "-1",
                "-p",
                &port.to_string(),
                "--forceflush",
                "--idle-timeout",
                "3",
                "--logfile",
            ])
            .arg(&server_log)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let marker = format!("Server listening on {port} ");
    wait_for(|| fs::read_to_string(&server_log).is_ok_and(|text| text.contains(&marker)));
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let mut client = OwnedChild(
        cli()
            .args([
                "-c",
                "127.0.0.1",
                "-p",
                &port.to_string(),
                "-t",
                "30",
                "-b",
                "1M",
                "-i",
                "0.1",
                "-J",
                "-I",
            ])
            .arg(&pidfile)
            .arg("--logfile")
            .arg(&jsonfile)
            .arg("--metrics.file")
            .arg(&metrics)
            .args([
                "--push.url",
                &endpoint,
                "--push.timeout",
                "5s",
                "--push.delete-on-exit",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for(|| {
        fs::read_to_string(&metrics).is_ok_and(|text| {
            text.lines().any(|line| {
                serde_json::from_str::<serde_json::Value>(line).is_ok_and(|record| {
                    record["transferred_bytes"]
                        .as_f64()
                        .is_some_and(|bytes| bytes > 0.0)
                })
            })
        })
    });
    let mut connection = accept_http(&listener);
    assert!(read_http(&mut connection).starts_with(b"PUT "));
    assert!(client.0.try_wait().unwrap().is_none());
    let interrupted = Instant::now();
    assert!(
        Command::new("kill")
            .args(["-TERM", &client.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    wait_for(|| {
        fs::read(&jsonfile)
            .is_ok_and(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).is_ok())
    });
    let json: serde_json::Value = serde_json::from_slice(&fs::read(&jsonfile).unwrap()).unwrap();
    assert!(json["error"].as_str().unwrap().contains("signal"));
    assert!(json["end"]["sum_sent"]["bytes"].as_u64().unwrap() > 0);
    assert!(json["end"]["sum_sent"]["seconds"].as_f64().unwrap() < 10.0);
    assert!(client.0.try_wait().unwrap().is_none() && pidfile.exists());
    release_http(connection);
    loop {
        let mut connection = accept_http(&listener);
        let request = read_http(&mut connection);
        let deleted = request.starts_with(b"DELETE ");
        assert!(deleted || request.starts_with(b"PUT "));
        release_http(connection);
        if deleted {
            break;
        }
    }
    wait_child(&mut client.0);
    assert!(interrupted.elapsed() < Duration::from_secs(8));
    assert!(!pidfile.exists());
    wait_child(&mut server.0);
    for file in [server_log, jsonfile, metrics] {
        fs::remove_file(file).unwrap();
    }
}
