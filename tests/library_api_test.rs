use std::time::Duration;
#[cfg(feature = "serde")]
use std::{
    env, fs,
    time::{SystemTime, UNIX_EPOCH},
};
#[cfg(all(feature = "pushgateway", feature = "serde"))]
use std::{
    io::{ErrorKind as IoErrorKind, Read, Write},
    net::TcpListener,
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread,
    time::Instant,
};

use iperf3_rs::{
    ErrorKind, IperfCommand, Metrics, MetricsMode, PrometheusEncoder, WindowMetrics,
    libiperf_version, usage_long,
};
#[cfg(all(feature = "pushgateway", feature = "serde"))]
use iperf3_rs::{MetricDirection, PushGatewayConfig, TransportProtocol};
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

#[cfg(all(feature = "pushgateway", feature = "serde"))]
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

#[cfg(all(feature = "pushgateway", feature = "serde"))]
fn run_library_client_blocking(port: u16, mode: MetricsMode) -> iperf3_rs::IperfResult {
    let mut last_error = String::new();
    for _ in 0..20 {
        match try_run_library_client_blocking(port, mode) {
            Ok(result) => return result,
            Err(err) => {
                last_error = err.to_string();
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    panic!("blocking client should complete: {last_error}");
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
fn try_run_library_client_blocking(
    port: u16,
    mode: MetricsMode,
) -> iperf3_rs::Result<iperf3_rs::IperfResult> {
    let mut command = IperfCommand::client("127.0.0.1");
    command
        .port(port)
        .duration(Duration::from_secs(1))
        .report_interval(Duration::from_secs(1))
        .json()
        .metrics(mode);

    command.run()
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
fn try_run_library_client(
    port: u16,
    mode: MetricsMode,
) -> iperf3_rs::Result<(iperf3_rs::IperfResult, Vec<MetricEvent>)> {
    let mut command = IperfCommand::client("127.0.0.1");
    command
        .port(port)
        .duration(Duration::from_secs(1))
        .report_interval(Duration::from_secs(1))
        .json();

    let (running, mut metrics) = command.spawn_with_metrics(mode)?;
    let events = metrics.by_ref().collect::<Vec<_>>();
    let result = running.wait()?;
    Ok((result, events))
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
fn try_run_library_direct_push_client(
    port: u16,
    config: PushGatewayConfig,
) -> iperf3_rs::Result<()> {
    let mut command = IperfCommand::client("127.0.0.1");
    command
        .port(port)
        .duration(Duration::from_secs(2))
        .report_interval(Duration::from_secs(1));

    command.run_with_pushgateway(config, MetricsMode::Interval)?;
    Ok(())
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
fn run_cli_metrics_file_client(port: u16, metrics_file: &Path, extra_args: &[&str]) -> Output {
    let mut last_output = None;
    for _ in 0..20 {
        let port = port.to_string();
        let metrics_file = metrics_file.to_string_lossy();
        let mut args = vec![
            "-c",
            "127.0.0.1",
            "-p",
            port.as_str(),
            "-t",
            "1",
            "-i",
            "1",
            "--metrics.file",
            metrics_file.as_ref(),
        ];
        args.extend_from_slice(extra_args);

        let output = Command::new(env!("CARGO_BIN_EXE_iperf3-rs"))
            .args(args)
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

#[cfg(all(feature = "pushgateway", feature = "serde"))]
fn free_loopback_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral loopback port");
    listener.local_addr().unwrap().port()
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
struct OneOffServer {
    child: Child,
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
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

#[cfg(all(feature = "pushgateway", feature = "serde"))]
impl Drop for OneOffServer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
struct OneShotHttpSink {
    handle: thread::JoinHandle<String>,
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
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

#[cfg(all(feature = "pushgateway", feature = "serde"))]
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

#[cfg(all(feature = "pushgateway", feature = "serde"))]
fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
fn content_length(headers: &[u8]) -> Option<usize> {
    String::from_utf8_lossy(headers).lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}

#[cfg(feature = "serde")]
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
