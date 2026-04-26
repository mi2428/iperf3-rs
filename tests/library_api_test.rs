use std::{
    env, fs,
    io::{ErrorKind as IoErrorKind, Read, Write},
    net::TcpListener,
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use iperf3_rs::{
    ErrorKind, IperfCommand, MetricEvent, MetricsMode, PushGatewayConfig, TransportProtocol,
    libiperf_version, usage_long,
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
                assert!(request.contains("iperf3_bytes"));
                assert!(request.contains("iperf3_bandwidth"));
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

#[test]
fn cli_writes_jsonl_metrics_file_without_replacing_stdout() {
    let port = free_loopback_port();
    let _server = OneOffServer::start(port);
    let metrics_file = temp_metrics_path("jsonl");

    let output = run_cli_metrics_file_client(port, &metrics_file);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[ ID]"));
    assert!(stdout.contains("sender"));

    let metrics = fs::read_to_string(&metrics_file).unwrap();
    assert!(
        metrics
            .lines()
            .any(|line| line.contains(r#""event":"interval""#))
    );
    assert!(metrics.contains(r#""bandwidth_bits_per_second":"#));
    let _ = fs::remove_file(metrics_file);
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
        .duration(Duration::from_secs(1))
        .report_interval(Duration::from_secs(1))
        .args(["--logfile", logfile.as_ref()]);

    let (running, mut metrics) = command.spawn_with_metrics(mode)?;
    let events = metrics.by_ref().collect::<Vec<_>>();
    let result = running.wait()?;
    Ok((result, events))
}

fn try_run_library_direct_push_client(
    port: u16,
    config: PushGatewayConfig,
) -> iperf3_rs::Result<()> {
    let logfile = env::temp_dir().join(format!("iperf3-rs-library-direct-push-{port}.log"));
    let logfile = logfile.to_string_lossy();
    let mut command = IperfCommand::client("127.0.0.1");
    command
        .port(port)
        .duration(Duration::from_secs(2))
        .report_interval(Duration::from_secs(1))
        .args(["--logfile", logfile.as_ref()]);

    command.run_with_pushgateway(config, MetricsMode::Interval)?;
    Ok(())
}

fn run_cli_metrics_file_client(port: u16, metrics_file: &Path) -> Output {
    let mut last_output = None;
    for _ in 0..20 {
        let output = Command::new(env!("CARGO_BIN_EXE_iperf3-rs"))
            .args([
                "-c",
                "127.0.0.1",
                "-p",
                &port.to_string(),
                "-t",
                "1",
                "-i",
                "1",
                "--metrics.file",
                metrics_file.to_string_lossy().as_ref(),
            ])
            .output()
            .expect("run iperf3-rs client with metrics file");
        if output.status.success() {
            return output;
        }
        last_output = Some(output);
        thread::sleep(Duration::from_millis(100));
    }

    let output = last_output.expect("client should have run at least once");
    panic!(
        "client should complete\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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

struct OneShotHttpSink {
    handle: thread::JoinHandle<String>,
}

impl OneShotHttpSink {
    fn start() -> (Self, String) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind HTTP sink");
        listener
            .set_nonblocking(true)
            .expect("set HTTP sink nonblocking");
        let endpoint = format!("http://{}", listener.local_addr().unwrap());

        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut first_request = None;
            let mut idle_deadline = None;
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("set HTTP stream blocking");
                        let request = read_http_request(&mut stream);
                        stream
                            .write_all(
                                b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .expect("write Pushgateway response");
                        first_request.get_or_insert(request);
                        idle_deadline = Some(Instant::now() + Duration::from_secs(1));
                    }
                    Err(err) if err.kind() == IoErrorKind::WouldBlock => {
                        match idle_deadline {
                            Some(deadline) if Instant::now() >= deadline => {
                                return first_request.expect("HTTP sink received a request");
                            }
                            _ => {}
                        }
                        assert!(Instant::now() < deadline, "timed out waiting for HTTP push");
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(err) => panic!("accept HTTP push: {err}"),
                }
            }
        });

        (Self { handle }, endpoint)
    }

    fn wait(self) -> String {
        self.handle.join().expect("HTTP sink thread should finish")
    }
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set HTTP sink read timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut header_end = None;

    while header_end.is_none() {
        let n = stream.read(&mut buffer).expect("read HTTP request headers");
        assert!(n > 0, "HTTP client closed before headers");
        request.extend_from_slice(&buffer[..n]);
        header_end = find_header_end(&request);
    }

    let header_end = header_end.unwrap();
    let content_length = content_length(&request[..header_end]).unwrap_or(0);
    while request.len() < header_end + 4 + content_length {
        let n = stream.read(&mut buffer).expect("read HTTP request body");
        assert!(n > 0, "HTTP client closed before body");
        request.extend_from_slice(&buffer[..n]);
    }

    String::from_utf8_lossy(&request).into_owned()
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(headers: &[u8]) -> Option<usize> {
    String::from_utf8_lossy(headers).lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}

fn temp_metrics_path(extension: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    env::temp_dir().join(format!(
        "iperf3-rs-cli-metrics-{}-{nonce}.{extension}",
        std::process::id()
    ))
}
