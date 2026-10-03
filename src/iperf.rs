//! Minimal Rust wrapper around upstream libiperf.
//!
//! Most users should prefer [`crate::IperfCommand`]. This module keeps the FFI
//! boundary localized and exposes only small value types at the crate root.

use std::ffi::{CStr, CString, c_void};
use std::os::raw::{c_char, c_double, c_int};
use std::ptr::NonNull;

use crate::{Error, ErrorKind, Result};

#[allow(non_camel_case_types)]
mod ffi {
    use super::{c_char, c_double, c_int, c_void};

    // libiperf owns this object; Rust only passes the opaque pointer back to C.
    #[repr(C)]
    pub struct iperf_test {
        _private: [u8; 0],
    }

    pub type MetricsCallback = unsafe extern "C" fn(
        *mut iperf_test,
        c_double,
        c_double,
        c_double,
        c_double,
        c_double,
        c_double,
        c_double,
        c_double,
        c_double,
        c_double,
        c_double,
        c_double,
        c_double,
        c_double,
        c_double,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
    );

    unsafe extern "C" {
        pub fn iperf_new_test() -> *mut iperf_test;
        pub fn iperf_defaults(test: *mut iperf_test) -> c_int;
        pub fn iperf_free_test(test: *mut iperf_test);
        pub fn iperf3rs_parse_arguments(
            test: *mut iperf_test,
            argc: c_int,
            argv: *mut *mut c_char,
        ) -> c_int;
        pub fn iperf3rs_clear_error_state();
        #[cfg(all(feature = "pushgateway", feature = "serde"))]
        pub fn iperf3rs_arg_boundary(
            word: *mut c_char,
            next: *mut c_char,
            info: *mut c_int,
        ) -> c_int;
        pub fn iperf_run_client(test: *mut iperf_test) -> c_int;
        pub fn iperf_reset_test(test: *mut iperf_test);
        pub fn iperf_get_test_role(test: *mut iperf_test) -> c_char;
        pub fn iperf_get_test_one_off(test: *mut iperf_test) -> c_int;
        pub fn iperf_get_test_json_output_string(test: *mut iperf_test) -> *const c_char;
        pub fn iperf_get_iperf_version() -> *const c_char;

        pub fn iperf3rs_enable_interval_metrics(
            test: *mut iperf_test,
            callback: Option<MetricsCallback>,
        );
        pub fn iperf3rs_run_server_once(test: *mut iperf_test) -> c_int;
        pub fn iperf3rs_suppress_output(test: *mut iperf_test) -> c_int;
        pub fn iperf3rs_current_errno() -> c_int;
        pub fn iperf3rs_is_auth_test_error() -> c_int;
        pub fn iperf3rs_current_error() -> *const c_char;
        pub fn iperf3rs_ignore_sigpipe() -> *mut c_void;
        pub fn iperf3rs_restore_sigpipe(saved: *mut c_void) -> c_int;
        pub fn iperf3rs_cli_interrupted() -> c_int;
        #[cfg(all(feature = "pushgateway", feature = "serde"))]
        pub fn iperf3rs_cli_prepare(test: *mut iperf_test) -> *mut c_void;
        #[cfg(all(feature = "pushgateway", feature = "serde"))]
        pub fn iperf3rs_cli_cleanup(state: *mut c_void) -> c_int;
        pub fn iperf3rs_usage_long() -> *mut c_char;
        pub fn iperf3rs_free_string(value: *mut c_char);
        #[cfg(test)]
        pub fn iperf3rs_diskfile_name(test: *mut iperf_test) -> *const c_char;
        #[cfg(test)]
        pub fn iperf3rs_sigpipe_probe(install: c_int) -> c_int;
        #[cfg(test)]
        pub fn iperf3rs_reorder_delta(
            current: std::os::raw::c_long,
            previous: std::os::raw::c_long,
        ) -> std::os::raw::c_long;
        #[cfg(test)]
        pub fn iperf3rs_json_probe(install: c_int) -> c_int;
        #[cfg(test)]
        pub fn iperf3rs_json_probe_session(test: *mut iperf_test, finish: c_int) -> c_int;
    }
}

pub(crate) use ffi::iperf_test as RawIperfTest;

#[cfg(all(feature = "pushgateway", feature = "serde"))]
pub(crate) fn arg_boundary(word: &str, next: Option<&str>) -> Result<(usize, i32)> {
    let bytes = |value: &str| {
        CString::new(value)
            .map(CString::into_bytes_with_nul)
            .map_err(|_| Error::invalid_argument("argument contains NUL"))
    };
    let mut word = bytes(word)?;
    let mut next = next.map(bytes).transpose()?;
    let next = next
        .as_mut()
        .map_or(std::ptr::null_mut(), |value| value.as_mut_ptr().cast());
    let mut info = 0;
    let span = unsafe { ffi::iperf3rs_arg_boundary(word.as_mut_ptr().cast(), next, &mut info) };
    Ok((span as usize, info))
}

/// Role selected by libiperf after parsing iperf arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub enum Role {
    /// Client mode, equivalent to `iperf3 -c`.
    Client,
    /// Server mode, equivalent to `iperf3 -s`.
    Server,
    /// A role byte libiperf returned that this crate does not recognize.
    Unknown(i8),
}

impl Default for Role {
    fn default() -> Self {
        Self::Unknown(0)
    }
}

pub struct IperfTest {
    ptr: NonNull<ffi::iperf_test>,
    // libiperf borrows and mutates argv bytes. Drop them only after native free.
    argv_storage: Vec<Vec<u8>>,
    retained_json: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseOutcome {
    Parsed,
    Help,
    Version,
    UsageError,
}

pub(crate) struct SigpipeGuard(Option<NonNull<c_void>>);

#[cfg(all(feature = "pushgateway", feature = "serde"))]
pub(crate) struct CliGuard(Option<NonNull<c_void>>);

#[cfg(all(feature = "pushgateway", feature = "serde"))]
impl CliGuard {
    pub(crate) fn prepare(test: &IperfTest) -> Result<Self> {
        NonNull::new(unsafe { ffi::iperf3rs_cli_prepare(test.as_ptr()) })
            .map(|state| Self(Some(state)))
            .ok_or_else(|| {
                Error::with_source(
                    ErrorKind::Libiperf,
                    format!("failed to prepare CLI runtime: {}", current_error()),
                    std::io::Error::last_os_error(),
                )
            })
    }

    pub(crate) fn cleanup(&mut self) -> Result<()> {
        if let Some(state) = self.0.take()
            && unsafe { ffi::iperf3rs_cli_cleanup(state.as_ptr()) } < 0
        {
            return Err(Error::with_source(
                ErrorKind::Libiperf,
                "failed to clean up CLI runtime",
                std::io::Error::last_os_error(),
            ));
        }
        Ok(())
    }
}

#[cfg(all(feature = "pushgateway", feature = "serde"))]
impl Drop for CliGuard {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("{error:#}");
        }
    }
}

impl SigpipeGuard {
    pub(crate) fn install() -> Result<Self> {
        NonNull::new(unsafe { ffi::iperf3rs_ignore_sigpipe() })
            .map(|saved| Self(Some(saved)))
            .ok_or_else(|| {
                Error::with_source(
                    ErrorKind::Libiperf,
                    "failed to save and ignore SIGPIPE",
                    std::io::Error::last_os_error(),
                )
            })
    }

    pub(crate) fn restore(&mut self) -> Result<()> {
        if let Some(saved) = self.0.take()
            && unsafe { ffi::iperf3rs_restore_sigpipe(saved.as_ptr()) } < 0
        {
            return Err(Error::with_source(
                ErrorKind::Libiperf,
                "failed to restore SIGPIPE",
                std::io::Error::last_os_error(),
            ));
        }
        Ok(())
    }
}

impl Drop for SigpipeGuard {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("{error:#}");
        }
    }
}

impl IperfTest {
    pub fn new() -> Result<Self> {
        let ptr = NonNull::new(unsafe { ffi::iperf_new_test() })
            .ok_or_else(|| Error::internal("iperf_new_test returned null"))?;
        let test = Self {
            ptr,
            argv_storage: Vec::new(),
            retained_json: None,
        };
        let rc = unsafe { ffi::iperf_defaults(test.as_ptr()) };
        if rc < 0 {
            return Err(Error::libiperf(format!(
                "iperf_defaults failed: {}",
                current_error()
            )));
        }
        Ok(test)
    }

    pub(crate) fn as_ptr(&self) -> *mut RawIperfTest {
        self.ptr.as_ptr()
    }

    pub(crate) fn parse_arguments(&mut self, args: &[String]) -> Result<ParseOutcome> {
        let argc = c_int::try_from(args.len())
            .map_err(|_| Error::invalid_argument("too many iperf arguments"))?;
        let storage = args
            .iter()
            .map(|arg| {
                CString::new(arg.as_str())
                    .map(CString::into_bytes_with_nul)
                    .map_err(|_| Error::invalid_argument(format!("argument contains NUL: {arg:?}")))
            })
            .collect::<Result<Vec<_>>>()?;
        let start = self.argv_storage.len();
        self.argv_storage.extend(storage);
        let mut argv = self.argv_storage[start..]
            .iter_mut()
            .map(|arg| arg.as_mut_ptr().cast::<c_char>())
            .collect::<Vec<_>>();
        argv.push(std::ptr::null_mut());

        let rc = unsafe { ffi::iperf3rs_parse_arguments(self.as_ptr(), argc, argv.as_mut_ptr()) };
        if rc < 0 {
            return Err(Error::libiperf(format!(
                "failed to parse iperf options: {}",
                current_error()
            )));
        }
        match rc {
            0 => Ok(ParseOutcome::Parsed),
            1 => Ok(ParseOutcome::Help),
            2 => Ok(ParseOutcome::Version),
            3 => Ok(ParseOutcome::UsageError),
            _ => Err(Error::internal("unknown native parser outcome")),
        }
    }

    pub(crate) fn enable_interval_metrics(&mut self, callback: ffi::MetricsCallback) {
        unsafe { ffi::iperf3rs_enable_interval_metrics(self.as_ptr(), Some(callback)) };
    }

    pub(crate) fn suppress_output(&mut self) -> Result<()> {
        let rc = unsafe { ffi::iperf3rs_suppress_output(self.as_ptr()) };
        if rc < 0 {
            return Err(Error::internal("failed to suppress libiperf output"));
        }
        Ok(())
    }

    pub fn role(&self) -> Role {
        match unsafe { ffi::iperf_get_test_role(self.as_ptr()) } as u8 as char {
            'c' => Role::Client,
            's' => Role::Server,
            other => Role::Unknown(other as i8),
        }
    }

    pub(crate) fn one_off(&self) -> bool {
        (unsafe { ffi::iperf_get_test_one_off(self.as_ptr()) }) != 0
    }

    /// Return libiperf's retained JSON result, when JSON output was requested.
    pub fn json_output(&self) -> Option<String> {
        self.retained_json.clone()
    }

    fn native_json_output(&self) -> Option<String> {
        let ptr = unsafe { ffi::iperf_get_test_json_output_string(self.as_ptr()) };
        if ptr.is_null() {
            return None;
        }
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    pub fn run(&mut self) -> Result<()> {
        let mut sigpipe = SigpipeGuard::install()?;
        let result = match self.role() {
            Role::Client => self.run_client(),
            Role::Server => self.run_server(),
            Role::Unknown(role) => Err(Error::invalid_argument(format!(
                "iperf role was not set by arguments: {role}"
            ))),
        };
        sigpipe.restore()?;
        result
    }

    fn run_client(&mut self) -> Result<()> {
        let rc = unsafe { ffi::iperf_run_client(self.as_ptr()) };
        self.retained_json = self.native_json_output();
        if unsafe { ffi::iperf3rs_cli_interrupted() } != 0 {
            return Ok(()); // Upstream treats CLI SIGINT/SIGTERM/SIGHUP as normal exit.
        }
        if rc < 0 {
            return Err(Error::libiperf(format!(
                "iperf client exited with error: {}",
                current_error()
            )));
        }
        Ok(())
    }

    fn run_server(&mut self) -> Result<()> {
        loop {
            // Upstream server mode handles one accepted test at a time and then
            // resets the same iperf_test so a long-running server can accept more.
            let rc = unsafe { ffi::iperf3rs_run_server_once(self.as_ptr()) };
            // Copy this session before reset releases C storage, including errors.
            self.retained_json = self.native_json_output();
            if unsafe { ffi::iperf3rs_cli_interrupted() } != 0 {
                return Ok(());
            }
            if rc < 0 {
                let error = current_error();
                if rc < -1 {
                    return Err(Error::libiperf(format!(
                        "iperf server exited with error: {error}"
                    )));
                }
                eprintln!("iperf server recovered from error: {error}");
            }

            unsafe { ffi::iperf_reset_test(self.as_ptr()) };

            let auth_error = unsafe { ffi::iperf3rs_is_auth_test_error() } != 0;
            if self.one_off() && rc != 2 {
                // Keep upstream's special-case behavior: authentication failures
                // in one-off mode should not terminate the server loop.
                if rc < 0 && auth_error {
                    continue;
                }
                return Ok(());
            }
        }
    }
}

impl Drop for IperfTest {
    fn drop(&mut self) {
        unsafe {
            ffi::iperf_free_test(self.as_ptr());
            // Error formatting has already copied any borrowed argv diagnostic.
            ffi::iperf3rs_clear_error_state();
        }
    }
}

pub(crate) fn current_error() -> String {
    let ptr = unsafe { ffi::iperf3rs_current_error() };
    if ptr.is_null() {
        let errno = unsafe { ffi::iperf3rs_current_errno() };
        return format!("unknown libiperf error ({errno})");
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Return the upstream libiperf version string.
pub fn libiperf_version() -> String {
    let ptr = unsafe { ffi::iperf_get_iperf_version() };
    if ptr.is_null() {
        return "unknown".to_owned();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Render the upstream iperf3 long help text.
///
/// The CLI combines this text with iperf3-rs-specific options before printing
/// `--help`.
pub fn usage_long() -> Result<String> {
    let ptr = unsafe { ffi::iperf3rs_usage_long() };
    if ptr.is_null() {
        return Err(Error::new(
            ErrorKind::Libiperf,
            "failed to render iperf usage text",
        ));
    }
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { ffi::iperf3rs_free_string(ptr) };
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_reorder_intervals_preserve_availability_and_resets() {
        for (current, previous, expected) in [
            (-1, 0, None),
            (0, 0, Some(0)),
            (2, 0, Some(2)),
            (2, 2, Some(0)),
            (5, 2, Some(3)),
            (1, 5, Some(1)),
            (3, -1, None),
        ] {
            let delta = unsafe { ffi::iperf3rs_reorder_delta(current, previous) };
            assert_eq!((delta >= 0).then_some(delta), expected);
        }
        let first_stream = unsafe { ffi::iperf3rs_reorder_delta(5, 2) };
        let second_stream = unsafe { ffi::iperf3rs_reorder_delta(7, 4) };
        assert_eq!(first_stream + second_stream, 6);
        assert_eq!(unsafe { ffi::iperf3rs_reorder_delta(2, 0) }, 2);
    }

    #[test]
    fn json_trees_and_retained_strings_are_released_on_reset_and_free() {
        const CHILD: &str = "IPERF3_RS_JSON_OWNER_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let _guard = crate::command::run_lock().lock().unwrap();
            assert_eq!(unsafe { ffi::iperf3rs_json_probe(1) }, 0);
            for finish in [0, 1] {
                let test = IperfTest::new().unwrap();
                for _ in 0..3 {
                    assert_eq!(
                        unsafe { ffi::iperf3rs_json_probe_session(test.as_ptr(), finish) },
                        0
                    );
                    if finish != 0 {
                        assert!(test.native_json_output().is_some());
                    }
                    unsafe { ffi::iperf_reset_test(test.as_ptr()) };
                    assert_eq!(unsafe { ffi::iperf3rs_json_probe(0) }, 0);
                    assert!(test.native_json_output().is_none());
                }
                assert_eq!(
                    unsafe { ffi::iperf3rs_json_probe_session(test.as_ptr(), 0) },
                    0
                );
                drop(test);
                assert_eq!(unsafe { ffi::iperf3rs_json_probe(0) }, 0);
            }
            println!("JSON_OWNER_RETURNED");
            return;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "iperf::tests::json_trees_and_retained_strings_are_released_on_reset_and_free",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("JSON ownership child timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("JSON_OWNER_RETURNED")
        );
    }

    #[test]
    fn parser_sets_server_role() {
        let _guard = crate::command::run_lock().lock().unwrap();
        let mut test = IperfTest::new().unwrap();
        test.parse_arguments(&["iperf3-rs".to_owned(), "-s".to_owned(), "-1".to_owned()])
            .unwrap();

        assert_eq!(test.role(), Role::Server);
        assert!(test.json_output().is_none());
    }

    #[test]
    fn parser_sets_client_role() {
        let _guard = crate::command::run_lock().lock().unwrap();
        let mut test = IperfTest::new().unwrap();
        test.parse_arguments(&[
            "iperf3-rs".to_owned(),
            "-c".to_owned(),
            "127.0.0.1".to_owned(),
            "-t".to_owned(),
            "1".to_owned(),
        ])
        .unwrap();

        assert_eq!(test.role(), Role::Client);
    }

    #[test]
    fn parser_retains_owned_writable_argument_bytes() {
        let _guard = crate::command::run_lock().lock().unwrap();
        let mut test = IperfTest::new().unwrap();
        test.parse_arguments(&[
            "iperf3-rs".to_owned(),
            "-c".to_owned(),
            "127.0.0.1".to_owned(),
            "-b".to_owned(),
            "1M/2".to_owned(),
            "-F".to_owned(),
            "probe.dat".to_owned(),
        ])
        .unwrap();

        let file_name = unsafe { ffi::iperf3rs_diskfile_name(test.as_ptr()) };
        assert_eq!(file_name, test.argv_storage[6].as_ptr().cast::<c_char>());
        assert_eq!(
            unsafe { CStr::from_ptr(file_name) }.to_bytes(),
            b"probe.dat"
        );
        assert_eq!(
            CStr::from_bytes_until_nul(&test.argv_storage[4])
                .unwrap()
                .to_bytes(),
            b"1M"
        );
    }

    #[test]
    fn failed_parse_does_not_poison_subsequent_native_parsing() {
        let _guard = crate::command::run_lock().lock().unwrap();
        for (option, value) in [("-p", "0"), ("-b", "invalid")] {
            let error = {
                let mut test = IperfTest::new().unwrap();
                test.parse_arguments(&[
                    "iperf3-rs".to_owned(),
                    "-c".to_owned(),
                    "127.0.0.1".to_owned(),
                    option.to_owned(),
                    value.to_owned(),
                ])
                .unwrap_err()
            };
            assert_eq!(error.kind(), ErrorKind::Libiperf);
            if option == "-b" {
                assert!(error.to_string().contains("invalid"));
            }

            let mut next = IperfTest::new().unwrap();
            next.parse_arguments(&[
                "iperf3-rs".to_owned(),
                "-c".to_owned(),
                "127.0.0.1".to_owned(),
                "-b".to_owned(),
                "1M".to_owned(),
            ])
            .unwrap();
            assert_eq!(next.role(), Role::Client);
        }
    }

    #[test]
    fn sigpipe_disposition_is_restored_on_return_and_drop() {
        const CHILD: &str = "IPERF3_RS_SIGPIPE_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let _guard = crate::command::run_lock().lock().unwrap();
            assert_eq!(unsafe { ffi::iperf3rs_sigpipe_probe(1) }, 0);
            {
                let _temporary = SigpipeGuard::install().unwrap();
                assert_eq!(unsafe { ffi::iperf3rs_sigpipe_probe(0) }, 0);
            }
            assert_eq!(unsafe { ffi::iperf3rs_sigpipe_probe(0) }, 1);
            let mut test = IperfTest::new().unwrap();
            assert!(test.run().is_err());
            assert_eq!(unsafe { ffi::iperf3rs_sigpipe_probe(0) }, 1);
            drop(test);
            drop(_guard);
            assert!(crate::IperfCommand::new().run().is_err());
            assert_eq!(unsafe { ffi::iperf3rs_sigpipe_probe(0) }, 1);
            return;
        }

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "iperf::tests::sigpipe_disposition_is_restored_on_return_and_drop",
            ])
            .env(CHILD, "1")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "signal policy child failed: {status}");
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("signal policy child timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
