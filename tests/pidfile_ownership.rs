#![cfg(unix)]

use std::{
    ffi::CString,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

unsafe extern "C" {
    fn iperf3rs_pidfile_probe(path: *const std::ffi::c_char, mode: i32) -> i32;
}

fn probe(path: &Path, mode: i32) -> std::io::Result<()> {
    let _ = iperf3_rs::libiperf_version();
    let path = CString::new(path.to_str().unwrap()).unwrap();
    let error = unsafe { iperf3rs_pidfile_probe(path.as_ptr(), mode) };
    if error == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(error))
    }
}

fn path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("pidfile-correction-{}-{name}", std::process::id()))
}

fn identity(path: &Path) -> (u64, u64) {
    let metadata = fs::metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}

#[test]
fn uncertain_pidfile_owners_keep_bytes_and_inode() {
    let path = path("uncertain.pid");
    // Every probe uses this process's own PID, or a test-only error observer.
    let original = std::process::id().to_string();
    for mode in [0, 1, 2, 3] {
        fs::write(&path, &original).unwrap();
        let before = identity(&path);
        let error = probe(&path, mode).unwrap_err();
        match mode {
            0 => assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists),
            1 | 3 => assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied),
            _ => assert!(error.to_string().contains("Input/output error")),
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert_eq!(identity(&path), before);
        fs::remove_file(&path).unwrap();
    }
    for invalid in [
        "",
        "0",
        "-1",
        "+1",
        "12garbage",
        "12\0",
        "999999999999999999999999999999999999",
    ] {
        fs::write(&path, invalid).unwrap();
        let before = identity(&path);
        assert_eq!(
            probe(&path, 0).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(fs::read(&path).unwrap(), invalid.as_bytes());
        assert_eq!(identity(&path), before);
        fs::remove_file(&path).unwrap();
    }
}

#[test]
fn absent_and_confirmed_stale_are_created_but_replacements_are_not_truncated() {
    let path = path("create.pid");
    probe(&path, 0).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        std::process::id().to_string()
    );
    let before = identity(&path);
    // ESRCH is supplied by the observer; no unknown PID is probed.
    fs::write(&path, format!("{}\n", std::process::id())).unwrap();
    probe(&path, 4).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        std::process::id().to_string()
    );
    assert_eq!(identity(&path), before);
    let replacement = path.with_file_name(format!(
        "{}.replacement",
        path.file_name().unwrap().to_str().unwrap()
    ));
    let observed = path.with_file_name(format!(
        "{}.observed",
        path.file_name().unwrap().to_str().unwrap()
    ));
    fs::write(&replacement, b"other-own-fixture").unwrap();
    let replacement_identity = identity(&replacement);
    assert_eq!(
        probe(&path, 5).unwrap_err().kind(),
        std::io::ErrorKind::ResourceBusy
    );
    assert_eq!(fs::read(&path).unwrap(), b"other-own-fixture");
    assert_eq!(identity(&path), replacement_identity);
    assert_eq!(identity(&observed), before);
    for fixture in [path, observed] {
        fs::remove_file(fixture).unwrap();
    }
}

#[test]
fn actual_write_only_read_permission_failure_is_fail_closed_when_enforced() {
    let path = path("write-only.pid");
    let original = std::process::id().to_string();
    fs::write(&path, &original).unwrap();
    let before = identity(&path);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o200)).unwrap();
    let permission_enforced = fs::File::open(&path).is_err();
    if permission_enforced {
        assert_eq!(
            probe(&path, 0).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    } else {
        eprintln!("write-only mode is readable in this environment; observer still covers EACCES");
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    assert_eq!(identity(&path), before);
    fs::remove_file(path).unwrap();
}
