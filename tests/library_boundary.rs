use std::{
    net::TcpListener,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use iperf3_rs::{ErrorKind, IperfCommand};

#[test]
fn library_parser_and_idle_paths_return_to_the_caller() {
    const CHILD: &str = "IPERF3_RS_BOUNDARY_TEST_CHILD";
    const SENTINEL: &str = "BOUNDARY_RETURNED";
    if std::env::var_os(CHILD).is_some() {
        for args in [
            vec!["--help"],
            vec!["--version"],
            vec!["--unknown-option"],
            vec!["--client"],
        ] {
            let mut command = IperfCommand::new();
            command.args(args);
            assert_eq!(
                command.run().unwrap_err().kind(),
                ErrorKind::InvalidArgument
            );
        }
        #[cfg(not(feature = "openssl"))]
        {
            let mut command = IperfCommand::new();
            command.args(["--username", "unused"]);
            assert_eq!(
                command.run().unwrap_err().kind(),
                ErrorKind::InvalidArgument
            );
        }
        let port = TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut server = IperfCommand::server_once();
        server.port(port).args(["--idle-timeout", "1"]);
        assert!(server.run().is_ok());
        println!("{SENTINEL}");
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "library_parser_and_idle_paths_return_to_the_caller",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("library boundary child timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains(SENTINEL));
}
