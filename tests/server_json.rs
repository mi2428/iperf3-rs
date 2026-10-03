#![cfg(all(unix, feature = "pushgateway", feature = "serde"))]

#[path = "integration/process.rs"]
mod process;

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::{fd::AsRawFd, unix::net::UnixStream},
    process::Command,
    thread,
    time::{Duration, Instant},
};

unsafe extern "C" {
    fn iperf3rs_server_json_sessions_probe(port: i32, reset_ready: i32) -> i32;
}

#[test]
fn failed_json_session_is_finalized_before_a_successful_session() {
    const CHILD: &str = "IPERF3_RS_SERVER_JSON_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let _ = iperf3_rs::libiperf_version(); // Link the crate's native adapter.
        let port = TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let (mut reset_reader, reset_writer) = UnixStream::pair().unwrap();
        reset_reader
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let peer = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                match TcpStream::connect(("127.0.0.1", port)) {
                    Ok(stream) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                        assert!(
                            Instant::now() < deadline,
                            "test-owned server did not listen"
                        );
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => panic!("test connection failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .write_all(b"123456789012345678901234567890123456\0")
                .unwrap();
            let mut state = [0];
            stream.read_exact(&mut state).unwrap();
            drop(stream); // Ordinary client EOF before supplying parameters.
            reset_reader.read_exact(&mut [0]).unwrap();
            let mut success = false;
            for _ in 0..20 {
                let output = Command::new(env!("CARGO_BIN_EXE_iperf3-rs"))
                    .env_clear()
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
                    .output()
                    .unwrap();
                if output.status.success() {
                    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                    assert!(json.get("end").is_some() && json.get("error").is_none());
                    success = true;
                    break;
                }
                let error = String::from_utf8(output.stderr).unwrap();
                assert!(
                    error.contains("unable to connect") && error.contains("Connection refused"),
                    "native second-session failure must not be retried: {error}"
                );
                thread::sleep(Duration::from_millis(20));
            }
            assert!(success, "second session did not complete");
        });
        assert_eq!(
            unsafe { iperf3rs_server_json_sessions_probe(port.into(), reset_writer.as_raw_fd()) },
            0
        );
        peer.join().unwrap();
        println!("SERVER_JSON_RETURNED");
        return;
    }
    let output = process::run_command(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "failed_json_session_is_finalized_before_a_successful_session",
                "--nocapture",
            ])
            .env(CHILD, "1"),
        Duration::from_secs(10),
    )
    .expect("test-owned JSON session subtree should complete within its budget");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8(output.stderr).unwrap()
    );
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("SERVER_JSON_RETURNED")
    );
}
