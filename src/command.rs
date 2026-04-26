//! High-level command API for running libiperf tests from Rust.
//!
//! `IperfCommand` deliberately accepts argv-style iperf arguments rather than a
//! typed Rust clone of every upstream option. This keeps compatibility anchored
//! to libiperf's own parser while still giving Rust callers structured results
//! and live metric streams.

use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Sender, bounded};

use crate::iperf::{IperfTest, Role};
use crate::metrics::{
    CallbackMetricsReporter, MetricEvent, MetricsMode, MetricsStream, metric_event_stream,
};
use crate::{Error, Result};

static RUN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Builder for running an iperf test through libiperf.
///
/// Arguments are the normal iperf arguments without `argv[0]`; `IperfCommand`
/// inserts a program name before passing them to `iperf_parse_arguments`.
///
/// # Examples
///
/// ```no_run
/// use iperf3_rs::{IperfCommand, Result};
///
/// fn main() -> Result<()> {
///     let mut command = IperfCommand::new();
///     command.args(["-c", "127.0.0.1", "-t", "5"]);
///
///     let result = command.run()?;
///     println!("{:?}", result.role());
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct IperfCommand {
    program: String,
    args: Vec<String>,
    metrics_mode: MetricsMode,
}

impl IperfCommand {
    /// Create a command with no iperf role selected yet.
    pub fn new() -> Self {
        Self {
            program: "iperf3-rs".to_owned(),
            args: Vec::new(),
            metrics_mode: MetricsMode::Disabled,
        }
    }

    /// Override the program name passed as `argv[0]` to libiperf.
    pub fn program(&mut self, program: impl Into<String>) -> &mut Self {
        self.program = program.into();
        self
    }

    /// Append one iperf argument.
    pub fn arg(&mut self, arg: impl Into<String>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    /// Append several iperf arguments.
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Enable or disable callback metrics for this run.
    pub fn metrics(&mut self, mode: MetricsMode) -> &mut Self {
        self.metrics_mode = mode;
        self
    }

    /// Run the iperf test to completion and collect metric events in memory.
    pub fn run(&mut self) -> Result<IperfResult> {
        run_command(self.clone(), None)
    }

    /// Run iperf on a worker thread and optionally stream metric events live.
    ///
    /// If metrics are enabled, call [`RunningIperf::take_metrics`] before
    /// [`RunningIperf::wait`] to consume live events.
    pub fn spawn(&mut self) -> Result<RunningIperf> {
        let command = self.clone();
        let (ready_tx, ready_rx) = bounded::<ReadyMessage>(1);
        let handle = thread::spawn(move || run_command(command, Some(ready_tx)));

        match ready_rx.recv() {
            Ok(Ok(metrics)) => Ok(RunningIperf { handle, metrics }),
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(Error::worker(err))
            }
            Err(err) => {
                let _ = handle.join();
                Err(Error::worker(format!(
                    "iperf worker exited before setup completed: {err}"
                )))
            }
        }
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.clone());
        argv.extend(self.args.iter().cloned());
        argv
    }
}

impl Default for IperfCommand {
    fn default() -> Self {
        Self::new()
    }
}

/// Completed result from a blocking or spawned iperf run.
#[derive(Debug)]
pub struct IperfResult {
    role: Role,
    json_output: Option<String>,
    metrics: Vec<MetricEvent>,
}

impl IperfResult {
    /// Role selected by libiperf after parsing the supplied arguments.
    pub fn role(&self) -> Role {
        self.role
    }

    /// Upstream JSON result if JSON output was requested and libiperf retained it.
    pub fn json_output(&self) -> Option<&str> {
        self.json_output.as_deref()
    }

    /// Metric events collected by `IperfCommand::run`.
    ///
    /// Spawned commands deliver live metrics through `RunningIperf` instead, so
    /// their completed result does not duplicate the stream contents.
    pub fn metrics(&self) -> &[MetricEvent] {
        &self.metrics
    }
}

/// Handle for an iperf run executing on a worker thread.
#[derive(Debug)]
pub struct RunningIperf {
    handle: JoinHandle<Result<IperfResult>>,
    metrics: Option<MetricsStream>,
}

impl RunningIperf {
    /// Borrow the live metric stream, if metrics were enabled.
    pub fn metrics(&self) -> Option<&MetricsStream> {
        self.metrics.as_ref()
    }

    /// Take ownership of the live metric stream.
    pub fn take_metrics(&mut self) -> Option<MetricsStream> {
        self.metrics.take()
    }

    /// Wait until the iperf worker exits.
    pub fn wait(self) -> Result<IperfResult> {
        self.handle
            .join()
            .map_err(|_| Error::worker("iperf worker thread panicked"))?
    }
}

type ReadyMessage = std::result::Result<Option<MetricsStream>, String>;

struct RunSetup {
    test: IperfTest,
    role: Role,
    callback: Option<CallbackMetricsReporter>,
    stream: Option<MetricsStream>,
    worker: Option<JoinHandle<()>>,
}

fn run_command(command: IperfCommand, ready: Option<Sender<ReadyMessage>>) -> Result<IperfResult> {
    let _guard = run_lock()
        .lock()
        .map_err(|_| Error::internal("libiperf run lock is poisoned"))?;

    let mut setup = match setup_run(command) {
        Ok(setup) => setup,
        Err(err) => {
            notify_ready(ready, Err(format!("{err:#}")));
            return Err(err);
        }
    };

    notify_ready(ready, Ok(setup.stream.take()));

    let result = setup.test.run();
    let json_output = setup.test.json_output();

    // Removing the callback first closes the raw metrics channel, allowing the
    // event worker to flush any final window and exit before the result returns.
    drop(setup.callback.take());
    if let Some(worker) = setup.worker.take() {
        let _ = worker.join();
    }

    let metrics = setup
        .stream
        .map(|stream| stream.collect())
        .unwrap_or_default();

    result?;
    Ok(IperfResult {
        role: setup.role,
        json_output,
        metrics,
    })
}

fn setup_run(command: IperfCommand) -> Result<RunSetup> {
    validate_metrics_mode(command.metrics_mode)?;

    let mut test = IperfTest::new()?;
    test.parse_arguments(&command.argv())?;
    let role = test.role();

    let (callback, stream, worker) = match command.metrics_mode.callback_queue() {
        Some(queue) => {
            let (callback, rx) = CallbackMetricsReporter::attach(&mut test, queue)?;
            let (stream, worker) = metric_event_stream(rx, command.metrics_mode);
            (Some(callback), Some(stream), Some(worker))
        }
        None => (None, None, None),
    };

    Ok(RunSetup {
        test,
        role,
        callback,
        stream,
        worker,
    })
}

fn notify_ready(ready: Option<Sender<ReadyMessage>>, message: ReadyMessage) {
    if let Some(ready) = ready {
        let _ = ready.send(message);
    }
}

fn run_lock() -> &'static Mutex<()> {
    // libiperf still has process-global state, including its current error and
    // signal/output hooks. The first public API keeps high-level runs
    // serialized so callers do not accidentally depend on best-effort
    // in-process parallelism that libiperf does not clearly promise.
    //
    // If parallel library runs become important, prefer adding a process-backed
    // runner first. A helper process gives each libiperf instance its own
    // globals while keeping this Rust API stable. Removing this lock for true
    // in-process concurrency should come only after upstream and shim state are
    // audited and covered by stress tests.
    RUN_LOCK.get_or_init(|| Mutex::new(()))
}

fn validate_metrics_mode(mode: MetricsMode) -> Result<()> {
    if metrics_mode_is_valid(mode) {
        Ok(())
    } else {
        Err(Error::invalid_metrics_mode(
            "metrics window interval must be greater than zero",
        ))
    }
}

fn metrics_mode_is_valid(mode: MetricsMode) -> bool {
    !matches!(mode, MetricsMode::Window(interval) if interval.is_zero())
}

#[cfg(kani)]
mod verification {
    use std::time::Duration;

    use super::*;

    #[kani::proof]
    fn zero_window_interval_is_the_only_invalid_metrics_mode() {
        let seconds: u8 = kani::any();
        let mode = MetricsMode::Window(Duration::from_secs(u64::from(seconds)));

        assert_eq!(metrics_mode_is_valid(mode), seconds != 0);
        assert!(metrics_mode_is_valid(MetricsMode::Disabled));
        assert!(metrics_mode_is_valid(MetricsMode::Interval));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::ErrorKind;

    use super::*;

    #[test]
    fn argv_includes_program_name_before_iperf_arguments() {
        let mut command = IperfCommand::new();
        command.arg("-c").arg("127.0.0.1");

        assert_eq!(
            command.argv(),
            vec![
                "iperf3-rs".to_owned(),
                "-c".to_owned(),
                "127.0.0.1".to_owned()
            ]
        );
    }

    #[test]
    fn custom_program_name_is_used_as_argv_zero() {
        let mut command = IperfCommand::new();
        command.program("iperf3").arg("-v");

        assert_eq!(command.argv(), vec!["iperf3".to_owned(), "-v".to_owned()]);
    }

    #[test]
    fn zero_metrics_window_interval_is_rejected_before_running_iperf() {
        let mut command = IperfCommand::new();
        command.metrics(MetricsMode::Window(Duration::ZERO));

        let err = command.run().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidMetricsMode);
        assert!(err.to_string().contains("greater than zero"));
    }

    #[test]
    fn run_without_client_or_server_role_fails_fast() {
        let mut command = IperfCommand::new();

        let err = command.run().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Libiperf);
        assert!(
            err.to_string().contains("client (-c) or server (-s)"),
            "{err:#}"
        );
    }
}
