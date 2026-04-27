#[cfg(feature = "serde")]
use std::{
    env,
    time::{SystemTime, UNIX_EPOCH},
};
#[cfg(all(feature = "pushgateway", feature = "serde"))]
use std::{
    io::{ErrorKind as IoErrorKind, Read, Write},
    net::TcpListener,
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(all(feature = "pushgateway", feature = "serde"))]
use iperf3_rs::{IperfCommand, MetricEvent, MetricsMode, PushGatewayConfig};

#[cfg(all(feature = "pushgateway", feature = "serde"))]
pub fn run_library_client(
    port: u16,
    mode: MetricsMode,
) -> (iperf3_rs::IperfResult, Vec<MetricEvent>) {
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
pub fn run_library_client_blocking(port: u16, mode: MetricsMode) -> iperf3_rs::IperfResult {
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
pub fn try_run_library_direct_push_client(
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
pub fn run_cli_metrics_file_client(port: u16, metrics_file: &Path, extra_args: &[&str]) -> Output {
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
pub fn free_loopback_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral loopback port");
    listener.local_addr().unwrap().port()
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
pub struct OneOffServer {
    child: Child,
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
impl OneOffServer {
    pub fn start(port: u16) -> Self {
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
pub struct OneShotHttpSink {
    handle: thread::JoinHandle<String>,
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
impl OneShotHttpSink {
    pub fn start() -> (Self, String) {
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

    pub fn wait(self) -> String {
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
pub fn temp_metrics_path(extension: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    env::temp_dir().join(format!(
        "iperf3-rs-cli-metrics-{}-{nonce}.{extension}",
        std::process::id()
    ))
}
