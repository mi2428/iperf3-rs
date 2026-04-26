//! Minimal Rust wrapper around upstream libiperf.
//!
//! Most users should prefer [`crate::IperfCommand`]. This module keeps the FFI
//! boundary localized and exposes only small value types at the crate root.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int};
use std::ptr::NonNull;

use crate::{Error, ErrorKind, Result};

#[allow(non_camel_case_types)]
mod ffi {
    use super::{c_char, c_double, c_int};

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
    );

    unsafe extern "C" {
        pub fn iperf_new_test() -> *mut iperf_test;
        pub fn iperf_defaults(test: *mut iperf_test) -> c_int;
        pub fn iperf_free_test(test: *mut iperf_test);
        pub fn iperf_parse_arguments(
            test: *mut iperf_test,
            argc: c_int,
            argv: *mut *mut c_char,
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
        pub fn iperf3rs_current_errno() -> c_int;
        pub fn iperf3rs_is_auth_test_error() -> c_int;
        pub fn iperf3rs_current_error() -> *const c_char;
        pub fn iperf3rs_ignore_sigpipe();
        pub fn iperf3rs_usage_long() -> *mut c_char;
        pub fn iperf3rs_free_string(value: *mut c_char);
    }
}

pub(crate) use ffi::iperf_test as RawIperfTest;

/// Role selected by libiperf after parsing iperf arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Client mode, equivalent to `iperf3 -c`.
    Client,
    /// Server mode, equivalent to `iperf3 -s`.
    Server,
    /// A role byte libiperf returned that this crate does not recognize.
    Unknown(i8),
}

pub struct IperfTest {
    ptr: NonNull<ffi::iperf_test>,
}

impl IperfTest {
    pub fn new() -> Result<Self> {
        let ptr = NonNull::new(unsafe { ffi::iperf_new_test() })
            .ok_or_else(|| Error::internal("iperf_new_test returned null"))?;
        let test = Self { ptr };
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

    pub fn parse_arguments(&mut self, args: &[String]) -> Result<()> {
        // libiperf parses synchronously, so the CString backing storage only
        // needs to stay alive for this call.
        let cstrings = args
            .iter()
            .map(|arg| {
                CString::new(arg.as_str())
                    .map_err(|_| Error::invalid_argument(format!("argument contains NUL: {arg:?}")))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut argv = cstrings
            .iter()
            .map(|arg| arg.as_ptr() as *mut c_char)
            .collect::<Vec<_>>();

        let rc = unsafe {
            ffi::iperf_parse_arguments(self.as_ptr(), argv.len() as c_int, argv.as_mut_ptr())
        };
        if rc < 0 {
            return Err(Error::libiperf(format!(
                "failed to parse iperf options: {}",
                current_error()
            )));
        }
        Ok(())
    }

    pub(crate) fn enable_interval_metrics(&mut self, callback: ffi::MetricsCallback) {
        unsafe { ffi::iperf3rs_enable_interval_metrics(self.as_ptr(), Some(callback)) };
    }

    pub fn role(&self) -> Role {
        match unsafe { ffi::iperf_get_test_role(self.as_ptr()) } as u8 as char {
            'c' => Role::Client,
            's' => Role::Server,
            other => Role::Unknown(other as i8),
        }
    }

    /// Return libiperf's retained JSON result, when JSON output was requested.
    pub fn json_output(&self) -> Option<String> {
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
        unsafe { ffi::iperf3rs_ignore_sigpipe() };
        match self.role() {
            Role::Client => self.run_client(),
            Role::Server => self.run_server(),
            Role::Unknown(role) => Err(Error::invalid_argument(format!(
                "iperf role was not set by arguments: {role}"
            ))),
        }
    }

    fn run_client(&mut self) -> Result<()> {
        let rc = unsafe { ffi::iperf_run_client(self.as_ptr()) };
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

            let one_off = unsafe { ffi::iperf_get_test_one_off(self.as_ptr()) } != 0;
            let auth_error = unsafe { ffi::iperf3rs_is_auth_test_error() } != 0;
            if one_off && rc != 2 {
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
        unsafe { ffi::iperf_free_test(self.as_ptr()) };
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
    use std::sync::Mutex;

    static IPERF_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parser_sets_server_role() {
        let _guard = IPERF_TEST_LOCK.lock().unwrap();
        let mut test = IperfTest::new().unwrap();
        test.parse_arguments(&["iperf3-rs".to_owned(), "-s".to_owned(), "-1".to_owned()])
            .unwrap();

        assert_eq!(test.role(), Role::Server);
        assert!(test.json_output().is_none());
    }

    #[test]
    fn parser_sets_client_role() {
        let _guard = IPERF_TEST_LOCK.lock().unwrap();
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
}
