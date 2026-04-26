//! High-level command API for running libiperf tests from Rust.
//!
//! `IperfCommand` deliberately accepts argv-style iperf arguments rather than a
//! typed Rust clone of every upstream option. This keeps compatibility anchored
//! to libiperf's own parser while still giving Rust callers structured results
//! and live metric streams.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, bounded};

use crate::iperf::{IperfTest, Role};
#[cfg(feature = "pushgateway")]
use crate::metrics::IntervalMetricsReporter;
use crate::metrics::{
    CallbackMetricsReporter, MetricEvent, MetricsMode, MetricsStream, metric_event_stream,
};
#[cfg(feature = "pushgateway")]
use crate::pushgateway::{PushGateway, PushGatewayConfig};
use crate::{Error, Result};

static RUN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Builder for running an iperf test through libiperf.
///
/// The typed helpers append ordinary iperf arguments such as `-c`, `-p`, and
/// `-t`. The lower-level [`IperfCommand::arg`] and [`IperfCommand::args`]
/// methods remain available for upstream options that do not have a dedicated
/// Rust helper.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use iperf3_rs::{IperfCommand, Result};
///
/// fn main() -> Result<()> {
///     let mut command = IperfCommand::client("127.0.0.1");
///     command.duration(Duration::from_secs(5));
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
    #[cfg(feature = "pushgateway")]
    pushgateway: Option<PushGatewayRun>,
    allow_unbounded_server: bool,
}

#[cfg(feature = "pushgateway")]
#[derive(Debug, Clone)]
struct PushGatewayRun {
    config: PushGatewayConfig,
    mode: MetricsMode,
}

impl IperfCommand {
    /// Create a command with no iperf role selected yet.
    pub fn new() -> Self {
        Self {
            program: "iperf3-rs".to_owned(),
            args: Vec::new(),
            metrics_mode: MetricsMode::Disabled,
            #[cfg(feature = "pushgateway")]
            pushgateway: None,
            allow_unbounded_server: false,
        }
    }

    /// Create a client command equivalent to `iperf3 -c HOST`.
    pub fn client(host: impl Into<String>) -> Self {
        let mut command = Self::new();
        command.arg("-c").arg(host);
        command
    }

    /// Create a one-off server command equivalent to `iperf3 -s -1`.
    ///
    /// This is the preferred server constructor for library code because the
    /// run exits after one accepted test and releases the process-wide libiperf
    /// lock.
    pub fn server_once() -> Self {
        let mut command = Self::new();
        command.args(["-s", "-1"]);
        command
    }

    /// Create a long-lived server command equivalent to `iperf3 -s`.
    ///
    /// Long-lived servers keep the high-level libiperf lock held until the
    /// server exits. Use this only for a process dedicated to serving tests.
    pub fn server_unbounded() -> Self {
        let mut command = Self::new();
        command.arg("-s").allow_unbounded_server(true);
        command
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

    /// Set the server port with iperf's `-p` option.
    pub fn port(&mut self, port: u16) -> &mut Self {
        self.arg("-p").arg(port.to_string())
    }

    /// Set client test duration with iperf's `-t` option.
    ///
    /// Upstream iperf parses `-t` as whole seconds. Sub-second durations are
    /// rounded up so the typed API does not silently truncate a nonzero
    /// [`Duration`] to `0`.
    pub fn duration(&mut self, duration: Duration) -> &mut Self {
        self.arg("-t").arg(whole_seconds_arg(duration))
    }

    /// Set reporting interval with iperf's `-i` option.
    pub fn report_interval(&mut self, interval: Duration) -> &mut Self {
        self.arg("-i").arg(decimal_seconds_arg(interval))
    }

    /// Send iperf output to a log file with iperf's `--logfile` option.
    pub fn logfile(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.arg("--logfile")
            .arg(path.as_ref().to_string_lossy().into_owned())
    }

    /// Set control connection setup timeout with iperf's `--connect-timeout`.
    ///
    /// Upstream iperf expects milliseconds. Nonzero sub-millisecond durations
    /// are rounded up so intent is not lost.
    pub fn connect_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.arg("--connect-timeout").arg(milliseconds_arg(timeout))
    }

    /// Omit pre-test statistics for the given duration with iperf's `-O`.
    pub fn omit(&mut self, duration: Duration) -> &mut Self {
        self.arg("-O").arg(decimal_seconds_arg(duration))
    }

    /// Bind to a local address or `address%device` with iperf's `-B`.
    pub fn bind(&mut self, address: impl Into<String>) -> &mut Self {
        self.arg("-B").arg(address)
    }

    /// Enable UDP mode with iperf's `-u` option.
    pub fn udp(&mut self) -> &mut Self {
        self.arg("-u")
    }

    /// Set target bitrate in bits per second with iperf's `-b` option.
    pub fn bitrate_bits_per_second(&mut self, bits_per_second: u64) -> &mut Self {
        self.arg("-b").arg(bits_per_second.to_string())
    }

    /// Set the number of parallel client streams with iperf's `-P` option.
    pub fn parallel_streams(&mut self, streams: u16) -> &mut Self {
        self.arg("-P").arg(streams.to_string())
    }

    /// Enable reverse mode with iperf's `-R` option.
    pub fn reverse(&mut self) -> &mut Self {
        self.arg("-R")
    }

    /// Enable bidirectional mode with iperf's `--bidir` option.
    pub fn bidirectional(&mut self) -> &mut Self {
        self.arg("--bidir")
    }

    /// Disable Nagle's algorithm with iperf's `-N` option.
    pub fn no_delay(&mut self) -> &mut Self {
        self.arg("-N")
    }

    /// Use zero-copy send with iperf's `-Z` option.
    pub fn zerocopy(&mut self) -> &mut Self {
        self.arg("-Z")
    }

    /// Set TCP congestion control algorithm with iperf's `-C` option.
    ///
    /// Upstream support depends on the operating system and linked libiperf
    /// build; unsupported values are reported by libiperf when the command
    /// parses or runs.
    pub fn congestion_control(&mut self, algorithm: impl Into<String>) -> &mut Self {
        self.arg("-C").arg(algorithm)
    }

    /// Request retained JSON output with iperf's `-J` option.
    pub fn json(&mut self) -> &mut Self {
        self.arg("-J")
    }

    /// Enable or disable callback metrics for this run.
    pub fn metrics(&mut self, mode: MetricsMode) -> &mut Self {
        self.metrics_mode = mode;
        self
    }

    /// Push live metrics for this run directly to a Pushgateway.
    ///
    /// `MetricsMode::Interval` uses the same freshness-oriented queue as the
    /// CLI's immediate push mode. `MetricsMode::Window(duration)` uses the same
    /// aggregation behavior as `--push.interval`. `MetricsMode::Disabled` is
    /// rejected when the command is started.
    ///
    /// Direct Pushgateway delivery and [`IperfCommand::spawn_with_metrics`] are
    /// currently mutually exclusive for one run because libiperf exposes a
    /// single reporter callback. Use [`IperfCommand::spawn_with_metrics`] plus
    /// [`PushGateway::push`] or [`PushGateway::push_window`] when application
    /// code needs both live inspection and custom push behavior.
    #[cfg(feature = "pushgateway")]
    pub fn pushgateway(&mut self, config: PushGatewayConfig, mode: MetricsMode) -> &mut Self {
        self.pushgateway = Some(PushGatewayRun { config, mode });
        self
    }

    /// Disable direct Pushgateway delivery for this command.
    #[cfg(feature = "pushgateway")]
    pub fn clear_pushgateway(&mut self) -> &mut Self {
        self.pushgateway = None;
        self
    }

    /// Run the iperf test to completion while pushing metrics to Pushgateway.
    #[cfg(feature = "pushgateway")]
    pub fn run_with_pushgateway(
        &mut self,
        config: PushGatewayConfig,
        mode: MetricsMode,
    ) -> Result<IperfResult> {
        let previous = self.pushgateway.replace(PushGatewayRun { config, mode });
        let result = self.run();
        self.pushgateway = previous;
        result
    }

    /// Run iperf on a worker thread while pushing metrics to Pushgateway.
    #[cfg(feature = "pushgateway")]
    pub fn spawn_with_pushgateway(
        &mut self,
        config: PushGatewayConfig,
        mode: MetricsMode,
    ) -> Result<RunningIperf> {
        let previous = self.pushgateway.replace(PushGatewayRun { config, mode });
        let result = self.spawn();
        self.pushgateway = previous;
        result
    }

    /// Allow `-s` server runs that do not include iperf's one-off option.
    ///
    /// Long-lived servers keep libiperf running on the worker thread and keep
    /// this crate's process-wide libiperf lock held. The default is therefore
    /// conservative: server mode must use `-1`/`--one-off` unless this opt-in is
    /// set. The CLI does not use this high-level API, so normal `iperf3-rs -s`
    /// behavior is unchanged.
    pub fn allow_unbounded_server(&mut self, allow: bool) -> &mut Self {
        self.allow_unbounded_server = allow;
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
            Ok(Ok(metrics)) => Ok(RunningIperf {
                handle: Some(handle),
                metrics,
            }),
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

    /// Run iperf on a worker thread and return the live metric stream.
    ///
    /// This is a convenience wrapper around [`IperfCommand::metrics`],
    /// [`IperfCommand::spawn`], and [`RunningIperf::take_metrics`] for callers
    /// that know they want metrics for this run.
    pub fn spawn_with_metrics(
        &mut self,
        mode: MetricsMode,
    ) -> Result<(RunningIperf, MetricsStream)> {
        self.metrics(mode);
        let mut running = self.spawn()?;
        let metrics = running
            .take_metrics()
            .ok_or_else(|| Error::internal("metrics stream was not created"))?;
        Ok((running, metrics))
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

    /// Parse the retained upstream JSON result as a [`serde_json::Value`].
    ///
    /// Returns `None` when JSON output was not requested with
    /// [`IperfCommand::json`]. The raw string remains available through
    /// [`IperfResult::json_output`] for callers that prefer their own parser.
    #[cfg(feature = "serde")]
    pub fn json_value(&self) -> Option<std::result::Result<serde_json::Value, serde_json::Error>> {
        self.json_output.as_deref().map(serde_json::from_str)
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
#[must_use = "dropping RunningIperf detaches the worker; call wait to observe the iperf result"]
pub struct RunningIperf {
    handle: Option<JoinHandle<Result<IperfResult>>>,
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

    /// Return `true` if the worker thread has finished.
    pub fn is_finished(&self) -> bool {
        self.handle
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(true)
    }

    /// Return the result if the worker has finished, without blocking.
    ///
    /// After this returns `Ok(Some(_))`, the worker result has been consumed and
    /// later calls to `try_wait`, `wait_timeout`, or `wait` will report that the
    /// run was already observed.
    pub fn try_wait(&mut self) -> Result<Option<IperfResult>> {
        if !self.is_finished() {
            return Ok(None);
        }
        self.take_finished_result().map(Some)
    }

    /// Wait up to `timeout` for the worker to finish.
    ///
    /// Returns `Ok(None)` when the timeout expires before the iperf run exits.
    /// A zero timeout performs a single nonblocking poll.
    pub fn wait_timeout(&mut self, timeout: Duration) -> Result<Option<IperfResult>> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        loop {
            if self.is_finished() {
                return self.take_finished_result().map(Some);
            }
            if timeout.is_zero() || Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }

    /// Wait until the iperf worker exits.
    pub fn wait(mut self) -> Result<IperfResult> {
        self.take_handle()?
            .join()
            .map_err(|_| Error::worker("iperf worker thread panicked"))?
    }

    fn take_finished_result(&mut self) -> Result<IperfResult> {
        self.take_handle()?
            .join()
            .map_err(|_| Error::worker("iperf worker thread panicked"))?
    }

    fn take_handle(&mut self) -> Result<JoinHandle<Result<IperfResult>>> {
        self.handle
            .take()
            .ok_or_else(|| Error::worker("iperf worker result was already observed"))
    }
}

type ReadyMessage = std::result::Result<Option<MetricsStream>, String>;

struct RunSetup {
    test: IperfTest,
    role: Role,
    callback: Option<CallbackMetricsReporter>,
    stream: Option<MetricsStream>,
    worker: Option<JoinHandle<()>>,
    #[cfg(feature = "pushgateway")]
    push_reporter: Option<IntervalMetricsReporter>,
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
    #[cfg(feature = "pushgateway")]
    let push_result = setup
        .push_reporter
        .take()
        .map(IntervalMetricsReporter::finish)
        .transpose();

    let metrics = setup
        .stream
        .map(|stream| stream.collect())
        .unwrap_or_default();

    result?;
    #[cfg(feature = "pushgateway")]
    push_result?;
    Ok(IperfResult {
        role: setup.role,
        json_output,
        metrics,
    })
}

fn setup_run(command: IperfCommand) -> Result<RunSetup> {
    validate_metrics_mode(command.metrics_mode)?;
    #[cfg(feature = "pushgateway")]
    validate_pushgateway_request(&command)?;

    let mut test = IperfTest::new()?;
    test.parse_arguments(&command.argv())?;
    let role = test.role();
    validate_server_lifecycle(&command, &test, role)?;

    #[cfg(feature = "pushgateway")]
    let (callback, stream, worker, push_reporter) =
        if let Some(queue) = command.metrics_mode.callback_queue() {
            let (callback, rx) = CallbackMetricsReporter::attach(&mut test, queue)?;
            let (stream, worker) = metric_event_stream(rx, command.metrics_mode);
            (Some(callback), Some(stream), Some(worker), None)
        } else if let Some(pushgateway) = command.pushgateway {
            let sink = PushGateway::new(pushgateway.config)?;
            let reporter =
                IntervalMetricsReporter::attach(&mut test, sink, pushgateway.mode.push_interval())?;
            (None, None, None, Some(reporter))
        } else {
            (None, None, None, None)
        };
    #[cfg(not(feature = "pushgateway"))]
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
        #[cfg(feature = "pushgateway")]
        push_reporter,
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

#[cfg(feature = "pushgateway")]
fn validate_pushgateway_request(command: &IperfCommand) -> Result<()> {
    let Some(pushgateway) = &command.pushgateway else {
        return Ok(());
    };
    if command.metrics_mode.is_enabled() {
        return Err(Error::invalid_argument(
            "direct Pushgateway delivery cannot be combined with a MetricsStream in the same IperfCommand run",
        ));
    }
    validate_pushgateway_mode(pushgateway.mode)
}

#[cfg(feature = "pushgateway")]
fn validate_pushgateway_mode(mode: MetricsMode) -> Result<()> {
    match mode {
        MetricsMode::Disabled => Err(Error::invalid_metrics_mode(
            "Pushgateway metrics mode must be Interval or Window",
        )),
        MetricsMode::Interval => Ok(()),
        MetricsMode::Window(interval) if interval.is_zero() => Err(Error::invalid_metrics_mode(
            "metrics window interval must be greater than zero",
        )),
        MetricsMode::Window(_) => Ok(()),
    }
}

#[cfg(feature = "pushgateway")]
impl MetricsMode {
    fn push_interval(self) -> Option<Duration> {
        match self {
            MetricsMode::Disabled | MetricsMode::Interval => None,
            MetricsMode::Window(interval) => Some(interval),
        }
    }
}

fn metrics_mode_is_valid(mode: MetricsMode) -> bool {
    !matches!(mode, MetricsMode::Window(interval) if interval.is_zero())
}

fn whole_seconds_arg(duration: Duration) -> String {
    let seconds = if duration.subsec_nanos() == 0 {
        duration.as_secs()
    } else {
        duration.as_secs().saturating_add(1)
    };
    seconds.to_string()
}

fn decimal_seconds_arg(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let nanos = duration.subsec_nanos();
    if nanos == 0 {
        return seconds.to_string();
    }

    let mut value = format!("{seconds}.{nanos:09}");
    while value.ends_with('0') {
        value.pop();
    }
    value
}

fn milliseconds_arg(duration: Duration) -> String {
    let millis = duration.as_millis();
    let has_fractional_millis = duration.subsec_nanos() % 1_000_000 != 0;
    if has_fractional_millis {
        millis.saturating_add(1).to_string()
    } else {
        millis.to_string()
    }
}

fn validate_server_lifecycle(command: &IperfCommand, test: &IperfTest, role: Role) -> Result<()> {
    if role == Role::Server && !test.one_off() && !command.allow_unbounded_server {
        return Err(Error::invalid_argument(
            "IperfCommand server mode must use -1/--one-off or opt in with allow_unbounded_server(true)",
        ));
    }
    Ok(())
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
    #[cfg(feature = "pushgateway")]
    use url::Url;

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
    fn typed_client_builder_appends_iperf_arguments() {
        let mut command = IperfCommand::client("192.0.2.10");
        command
            .port(5202)
            .duration(Duration::from_secs(3))
            .report_interval(Duration::from_millis(500))
            .udp()
            .bitrate_bits_per_second(1_000_000)
            .parallel_streams(4)
            .reverse()
            .json()
            .arg("--get-server-output");

        assert_eq!(
            command.argv(),
            vec![
                "iperf3-rs".to_owned(),
                "-c".to_owned(),
                "192.0.2.10".to_owned(),
                "-p".to_owned(),
                "5202".to_owned(),
                "-t".to_owned(),
                "3".to_owned(),
                "-i".to_owned(),
                "0.5".to_owned(),
                "-u".to_owned(),
                "-b".to_owned(),
                "1000000".to_owned(),
                "-P".to_owned(),
                "4".to_owned(),
                "-R".to_owned(),
                "-J".to_owned(),
                "--get-server-output".to_owned(),
            ]
        );
    }

    #[test]
    fn typed_operational_helpers_append_iperf_arguments() {
        let mut command = IperfCommand::client("192.0.2.10");
        command
            .logfile("iperf.log")
            .connect_timeout(Duration::from_millis(1500))
            .omit(Duration::from_millis(250))
            .bind("127.0.0.1%lo0")
            .no_delay()
            .zerocopy()
            .congestion_control("cubic");

        assert_eq!(
            command.argv(),
            vec![
                "iperf3-rs".to_owned(),
                "-c".to_owned(),
                "192.0.2.10".to_owned(),
                "--logfile".to_owned(),
                "iperf.log".to_owned(),
                "--connect-timeout".to_owned(),
                "1500".to_owned(),
                "-O".to_owned(),
                "0.25".to_owned(),
                "-B".to_owned(),
                "127.0.0.1%lo0".to_owned(),
                "-N".to_owned(),
                "-Z".to_owned(),
                "-C".to_owned(),
                "cubic".to_owned(),
            ]
        );
    }

    #[test]
    fn typed_server_constructors_select_expected_lifecycle() {
        let one_off = IperfCommand::server_once();
        assert_eq!(
            one_off.argv(),
            vec!["iperf3-rs".to_owned(), "-s".to_owned(), "-1".to_owned()]
        );
        assert!(!one_off.allow_unbounded_server);

        let unbounded = IperfCommand::server_unbounded();
        assert_eq!(
            unbounded.argv(),
            vec!["iperf3-rs".to_owned(), "-s".to_owned()]
        );
        assert!(unbounded.allow_unbounded_server);
    }

    #[test]
    fn bidirectional_helper_appends_long_option() {
        let mut command = IperfCommand::client("192.0.2.10");
        command.bidirectional();

        assert_eq!(
            command.argv(),
            vec![
                "iperf3-rs".to_owned(),
                "-c".to_owned(),
                "192.0.2.10".to_owned(),
                "--bidir".to_owned()
            ]
        );
    }

    #[cfg(feature = "pushgateway")]
    #[test]
    fn pushgateway_helper_records_delivery_config() {
        let config = PushGatewayConfig::new(Url::parse("http://localhost:9091").unwrap())
            .label("scenario", "library");
        let mut command = IperfCommand::client("192.0.2.10");
        command.pushgateway(config, MetricsMode::Window(Duration::from_secs(5)));

        let pushgateway = command.pushgateway.as_ref().unwrap();
        assert_eq!(
            pushgateway.mode,
            MetricsMode::Window(Duration::from_secs(5))
        );
        assert_eq!(
            pushgateway.config.labels,
            [("scenario".to_owned(), "library".to_owned())]
        );

        command.clear_pushgateway();
        assert!(command.pushgateway.is_none());
    }

    #[cfg(feature = "pushgateway")]
    #[test]
    fn pushgateway_convenience_helpers_do_not_persist_config() {
        let mut command = IperfCommand::new();
        command.metrics(MetricsMode::Window(Duration::ZERO));

        let result = command.run_with_pushgateway(
            PushGatewayConfig::new(Url::parse("http://localhost:9091").unwrap()),
            MetricsMode::Interval,
        );

        assert!(result.is_err());
        assert!(command.pushgateway.is_none());
    }

    #[test]
    fn duration_helpers_preserve_nonzero_subsecond_intent() {
        assert_eq!(whole_seconds_arg(Duration::ZERO), "0");
        assert_eq!(whole_seconds_arg(Duration::from_millis(1)), "1");
        assert_eq!(whole_seconds_arg(Duration::from_millis(1500)), "2");
        assert_eq!(decimal_seconds_arg(Duration::ZERO), "0");
        assert_eq!(decimal_seconds_arg(Duration::from_millis(250)), "0.25");
        assert_eq!(decimal_seconds_arg(Duration::new(1, 1)), "1.000000001");
        assert_eq!(milliseconds_arg(Duration::ZERO), "0");
        assert_eq!(milliseconds_arg(Duration::from_nanos(1)), "1");
        assert_eq!(milliseconds_arg(Duration::from_millis(1500)), "1500");
        assert_eq!(milliseconds_arg(Duration::new(1, 1)), "1001");
    }

    #[test]
    fn unbounded_server_mode_is_rejected_by_default() {
        let command = {
            let mut command = IperfCommand::new();
            command.arg("-s");
            command
        };

        let err = match setup_run(command) {
            Ok(_) => panic!("unbounded server should be rejected"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
        assert!(err.to_string().contains("allow_unbounded_server"));
    }

    #[test]
    fn one_off_server_mode_is_allowed() {
        let command = {
            let mut command = IperfCommand::new();
            command.args(["-s", "-1"]);
            command
        };

        let setup = setup_run(command).unwrap();
        assert_eq!(setup.role, Role::Server);
    }

    #[test]
    fn unbounded_server_mode_can_be_explicitly_allowed() {
        let command = {
            let mut command = IperfCommand::new();
            command.arg("-s").allow_unbounded_server(true);
            command
        };

        let setup = setup_run(command).unwrap();
        assert_eq!(setup.role, Role::Server);
    }

    #[test]
    fn zero_metrics_window_interval_is_rejected_before_running_iperf() {
        let mut command = IperfCommand::new();
        command.metrics(MetricsMode::Window(Duration::ZERO));

        let err = command.run().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidMetricsMode);
        assert!(err.to_string().contains("greater than zero"));
    }

    #[cfg(feature = "pushgateway")]
    #[test]
    fn direct_pushgateway_rejects_disabled_or_zero_window_mode() {
        for mode in [MetricsMode::Disabled, MetricsMode::Window(Duration::ZERO)] {
            let command = {
                let mut command = IperfCommand::new();
                command.arg("-s").arg("-1").pushgateway(
                    PushGatewayConfig::new(Url::parse("http://localhost:9091").unwrap()),
                    mode,
                );
                command
            };

            let err = match setup_run(command) {
                Ok(_) => panic!("invalid Pushgateway mode should be rejected"),
                Err(err) => err,
            };
            assert_eq!(err.kind(), ErrorKind::InvalidMetricsMode);
        }
    }

    #[cfg(feature = "pushgateway")]
    #[test]
    fn direct_pushgateway_is_rejected_when_metrics_stream_is_enabled() {
        let command = {
            let mut command = IperfCommand::new();
            command
                .arg("-s")
                .arg("-1")
                .metrics(MetricsMode::Interval)
                .pushgateway(
                    PushGatewayConfig::new(Url::parse("http://localhost:9091").unwrap()),
                    MetricsMode::Interval,
                );
            command
        };

        let err = match setup_run(command) {
            Ok(_) => panic!("direct Pushgateway and MetricsStream should be rejected together"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
        assert!(err.to_string().contains("cannot be combined"));
    }

    #[test]
    fn running_iperf_try_wait_observes_finished_worker_once() {
        let mut running = RunningIperf {
            handle: Some(thread::spawn(|| Ok(test_result()))),
            metrics: None,
        };

        let result = running
            .wait_timeout(Duration::from_secs(1))
            .unwrap()
            .expect("worker should finish");
        assert_eq!(result.role(), Role::Client);
        assert_eq!(running.try_wait().unwrap_err().kind(), ErrorKind::Worker);
    }

    #[test]
    fn running_iperf_try_wait_returns_none_while_worker_is_running() {
        let (release_tx, release_rx) = bounded::<()>(1);
        let mut running = RunningIperf {
            handle: Some(thread::spawn(move || {
                release_rx.recv().unwrap();
                Ok(test_result())
            })),
            metrics: None,
        };

        assert!(!running.is_finished());
        assert!(running.try_wait().unwrap().is_none());
        assert!(running.wait_timeout(Duration::ZERO).unwrap().is_none());

        release_tx.send(()).unwrap();
        assert!(
            running
                .wait_timeout(Duration::from_secs(1))
                .unwrap()
                .is_some()
        );
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

    fn test_result() -> IperfResult {
        IperfResult {
            role: Role::Client,
            json_output: None,
            metrics: Vec::new(),
        }
    }
}
