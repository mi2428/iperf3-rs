//! Metric structures and streams produced from libiperf interval callbacks.

use std::collections::HashMap;
use std::os::raw::c_double;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, bounded, unbounded};

use crate::iperf::{IperfTest, RawIperfTest};
use crate::pushgateway::PushGateway;

#[derive(Debug, Clone, Default, PartialEq)]
/// One libiperf interval sample.
///
/// Fields are normalized to Prometheus-friendly units where practical. TCP
/// fields are zero when libiperf or the operating system does not report them;
/// UDP fields are zero for normal TCP tests.
pub struct Metrics {
    /// Bytes transferred during the interval.
    pub bytes: f64,
    /// Interval throughput in bits per second.
    pub bandwidth_bits_per_second: f64,
    /// TCP retransmits reported for the interval.
    pub tcp_retransmits: f64,
    /// TCP smoothed RTT in seconds.
    pub tcp_rtt_seconds: f64,
    /// TCP RTT variance in seconds.
    pub tcp_rttvar_seconds: f64,
    /// TCP sender congestion window in bytes.
    pub tcp_snd_cwnd_bytes: f64,
    /// TCP sender window in bytes when available.
    pub tcp_snd_wnd_bytes: f64,
    /// TCP path MTU in bytes when available.
    pub tcp_pmtu_bytes: f64,
    /// TCP reordering events when available.
    pub tcp_reorder_events: f64,
    /// UDP packet count reported for the interval.
    pub udp_packets: f64,
    /// UDP packets inferred lost from sequence gaps.
    pub udp_lost_packets: f64,
    /// UDP receiver jitter in seconds.
    pub udp_jitter_seconds: f64,
    /// UDP out-of-order packets observed in the interval.
    pub udp_out_of_order_packets: f64,
    /// Interval duration in seconds.
    pub interval_duration_seconds: f64,
    /// `1` for omitted warm-up intervals, otherwise `0`.
    pub omitted: f64,
}

/// Mean, minimum, and maximum values for a gauge-like metric in a window.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WindowGaugeStats {
    /// Arithmetic mean over samples in the window.
    pub mean: f64,
    /// Minimum observed value in the window.
    pub min: f64,
    /// Maximum observed value in the window.
    pub max: f64,
}

/// Summary of one aggregated metrics window.
///
/// Counter-like fields are accumulated across the window. Gauge-like fields use
/// [`WindowGaugeStats`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowMetrics {
    /// Total interval duration represented by this window.
    pub duration_seconds: f64,
    /// Total bytes transferred across this window.
    pub transferred_bytes: f64,
    /// Bandwidth statistics in bytes per second.
    pub bandwidth_bytes_per_second: WindowGaugeStats,
    /// TCP smoothed RTT statistics in seconds.
    pub tcp_rtt_seconds: WindowGaugeStats,
    /// TCP RTT variance statistics in seconds.
    pub tcp_rttvar_seconds: WindowGaugeStats,
    /// TCP sender congestion window statistics in bytes.
    pub tcp_snd_cwnd_bytes: WindowGaugeStats,
    /// TCP sender window statistics in bytes.
    pub tcp_snd_wnd_bytes: WindowGaugeStats,
    /// TCP path MTU statistics in bytes.
    pub tcp_pmtu_bytes: WindowGaugeStats,
    /// UDP jitter statistics in seconds.
    pub udp_jitter_seconds: WindowGaugeStats,
    /// TCP retransmits accumulated across the window.
    pub tcp_retransmits: f64,
    /// TCP reordering events accumulated across the window.
    pub tcp_reorder_events: f64,
    /// UDP packet count accumulated across the window.
    pub udp_packets: f64,
    /// UDP lost packet count accumulated across the window.
    pub udp_lost_packets: f64,
    /// UDP out-of-order packet count accumulated across the window.
    pub udp_out_of_order_packets: f64,
    /// Number of omitted libiperf intervals in the window.
    pub omitted_intervals: f64,
}

/// Controls whether a run emits live metrics and how interval samples are shaped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetricsMode {
    /// Do not register the libiperf interval callback.
    #[default]
    Disabled,
    /// Emit one event for every libiperf interval sample.
    Interval,
    /// Aggregate interval samples into fixed-duration summary windows.
    Window(Duration),
}

impl MetricsMode {
    /// Return `true` when this mode installs the libiperf metrics callback.
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    pub(crate) const fn callback_queue(self) -> Option<MetricsQueue> {
        match self {
            Self::Disabled => None,
            // Library consumers should receive every sample. The freshness-only
            // replacement queue is reserved for immediate Pushgateway writes.
            Self::Interval | Self::Window(_) => Some(MetricsQueue::All),
        }
    }
}

/// Metric event emitted by a running iperf test.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MetricEvent {
    /// A raw libiperf interval sample.
    Interval(Metrics),
    /// A summary produced from one or more interval samples.
    Window(WindowMetrics),
}

/// Receiver for metric events emitted by a running iperf test.
#[derive(Debug)]
pub struct MetricsStream {
    rx: Receiver<MetricEvent>,
}

impl MetricsStream {
    fn new(rx: Receiver<MetricEvent>) -> Self {
        Self { rx }
    }

    /// Block until the next metric event arrives or the run ends.
    pub fn recv(&self) -> Option<MetricEvent> {
        self.rx.recv().ok()
    }

    /// Wait for the next metric event up to `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<MetricEvent> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// Return the next metric event if one is already queued.
    pub fn try_recv(&self) -> Option<MetricEvent> {
        self.rx.try_recv().ok()
    }
}

impl Iterator for MetricsStream {
    type Item = MetricEvent;

    fn next(&mut self) -> Option<Self::Item> {
        self.recv()
    }
}

pub(crate) struct IntervalMetricsReporter {
    callback: Option<CallbackMetricsReporter>,
    worker: Option<JoinHandle<()>>,
}

impl IntervalMetricsReporter {
    pub(crate) fn attach(
        test: &mut IperfTest,
        sink: PushGateway,
        push_interval: Option<Duration>,
    ) -> Result<Self> {
        let queue = if push_interval.is_some() {
            MetricsQueue::All
        } else {
            MetricsQueue::Latest
        };
        let (callback, rx) = CallbackMetricsReporter::attach(test, queue)?;

        // Network I/O happens off the libiperf callback path so slow or
        // unavailable Pushgateway writes do not stall the iperf test itself.
        let worker = thread::spawn(move || match push_interval {
            Some(interval) => push_window_metrics(rx, sink, interval),
            None => push_interval_metrics(rx, sink),
        });

        Ok(Self {
            callback: Some(callback),
            worker: Some(worker),
        })
    }
}

pub(crate) struct CallbackMetricsReporter {
    test_key: usize,
}

impl CallbackMetricsReporter {
    pub(crate) fn attach(
        test: &mut IperfTest,
        queue: MetricsQueue,
    ) -> Result<(Self, Receiver<Metrics>)> {
        let (target, rx) = callback_channel(queue);
        let test_key = test.as_ptr() as usize;
        callbacks()
            .lock()
            .map_err(|_| anyhow!("metrics callback registry is poisoned"))?
            .insert(test_key, target);

        test.enable_interval_metrics(metrics_callback);

        Ok((Self { test_key }, rx))
    }
}

impl Drop for CallbackMetricsReporter {
    fn drop(&mut self) {
        if let Ok(mut callbacks) = callbacks().lock() {
            callbacks.remove(&self.test_key);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetricsQueue {
    Latest,
    All,
}

fn callback_channel(queue: MetricsQueue) -> (CallbackTarget, Receiver<Metrics>) {
    match queue {
        MetricsQueue::All => {
            // Window aggregation and library streams need every libiperf
            // interval sample, so use an unbounded channel.
            let (tx, rx) = unbounded::<Metrics>();
            (
                CallbackTarget {
                    tx,
                    latest_rx: None,
                },
                rx,
            )
        }
        MetricsQueue::Latest => {
            // Pushgateway stores only the latest value for a grouping key.
            // Keep the callback nonblocking and replace stale queued samples if
            // HTTP writes fall behind.
            let (tx, rx) = bounded::<Metrics>(1);
            (
                CallbackTarget {
                    tx,
                    latest_rx: Some(rx.clone()),
                },
                rx,
            )
        }
    }
}

pub(crate) fn metric_event_stream(
    rx: Receiver<Metrics>,
    mode: MetricsMode,
) -> (MetricsStream, JoinHandle<()>) {
    let (tx, event_rx) = unbounded::<MetricEvent>();
    let worker = thread::spawn(move || match mode {
        MetricsMode::Disabled => {}
        MetricsMode::Interval => forward_interval_events(rx, tx),
        MetricsMode::Window(interval) => forward_window_events(rx, tx, interval),
    });
    (MetricsStream::new(event_rx), worker)
}

fn forward_interval_events(rx: Receiver<Metrics>, tx: Sender<MetricEvent>) {
    for metrics in rx {
        if tx.send(MetricEvent::Interval(metrics)).is_err() {
            break;
        }
    }
}

fn forward_window_events(rx: Receiver<Metrics>, tx: Sender<MetricEvent>, interval: Duration) {
    let mut window = Vec::new();
    let mut deadline = None;

    loop {
        match deadline {
            Some(flush_at) => {
                let now = Instant::now();
                if now >= flush_at {
                    if !flush_window_event(&tx, &mut window) {
                        break;
                    }
                    deadline = None;
                    continue;
                }

                match rx.recv_timeout(flush_at - now) {
                    Ok(metrics) => window.push(metrics),
                    Err(RecvTimeoutError::Timeout) => {
                        if !flush_window_event(&tx, &mut window) {
                            break;
                        }
                        deadline = None;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match rx.recv() {
                Ok(metrics) => {
                    window.push(metrics);
                    deadline = Some(
                        Instant::now()
                            .checked_add(interval)
                            .unwrap_or_else(Instant::now),
                    );
                }
                Err(_) => break,
            },
        }
    }

    let _ = flush_window_event(&tx, &mut window);
}

fn flush_window_event(tx: &Sender<MetricEvent>, window: &mut Vec<Metrics>) -> bool {
    let Some(metrics) = aggregate_window(window) else {
        return true;
    };
    window.clear();
    tx.send(MetricEvent::Window(metrics)).is_ok()
}

fn push_interval_metrics(rx: Receiver<Metrics>, sink: PushGateway) {
    for metrics in rx {
        if let Err(err) = sink.push(&metrics) {
            eprintln!("failed to push metrics: {err:#}");
        }
    }
}

fn push_window_metrics(rx: Receiver<Metrics>, sink: PushGateway, interval: Duration) {
    let mut window = Vec::new();
    let mut deadline = None;

    loop {
        match deadline {
            Some(flush_at) => {
                let now = Instant::now();
                if now >= flush_at {
                    flush_window_metrics(&sink, &mut window);
                    deadline = None;
                    continue;
                }

                match rx.recv_timeout(flush_at - now) {
                    Ok(metrics) => window.push(metrics),
                    Err(RecvTimeoutError::Timeout) => {
                        flush_window_metrics(&sink, &mut window);
                        deadline = None;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match rx.recv() {
                Ok(metrics) => {
                    window.push(metrics);
                    deadline = Some(
                        Instant::now()
                            .checked_add(interval)
                            .unwrap_or_else(Instant::now),
                    );
                }
                Err(_) => break,
            },
        }
    }

    // The final iperf interval often arrives shortly before the process exits.
    // Flush a partial window so short tests still publish useful summaries.
    flush_window_metrics(&sink, &mut window);
}

fn flush_window_metrics(sink: &PushGateway, window: &mut Vec<Metrics>) {
    if let Some(metrics) = aggregate_window(window) {
        if let Err(err) = sink.push_window(&metrics) {
            eprintln!("failed to push window metrics: {err:#}");
        }
        window.clear();
    }
}

impl Drop for IntervalMetricsReporter {
    fn drop(&mut self) {
        drop(self.callback.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct CallbackTarget {
    tx: Sender<Metrics>,
    latest_rx: Option<Receiver<Metrics>>,
}

static CALLBACKS: OnceLock<Mutex<HashMap<usize, CallbackTarget>>> = OnceLock::new();

fn callbacks() -> &'static Mutex<HashMap<usize, CallbackTarget>> {
    // The same extern callback is registered for every test, so dispatch by the
    // iperf_test pointer passed back from C.
    CALLBACKS.get_or_init(|| Mutex::new(HashMap::new()))
}

unsafe extern "C" fn metrics_callback(
    test: *mut RawIperfTest,
    bytes: c_double,
    bandwidth_bits_per_second: c_double,
    tcp_retransmits: c_double,
    tcp_rtt_seconds: c_double,
    tcp_rttvar_seconds: c_double,
    tcp_snd_cwnd_bytes: c_double,
    tcp_snd_wnd_bytes: c_double,
    tcp_pmtu_bytes: c_double,
    tcp_reorder_events: c_double,
    udp_packets: c_double,
    udp_lost_packets: c_double,
    udp_jitter_seconds: c_double,
    udp_out_of_order_packets: c_double,
    interval_duration_seconds: c_double,
    omitted: c_double,
) {
    if test.is_null() {
        return;
    }

    let Ok(callbacks) = callbacks().lock() else {
        return;
    };
    let Some(target) = callbacks.get(&(test as usize)) else {
        return;
    };

    enqueue_latest(
        target,
        Metrics {
            bytes,
            bandwidth_bits_per_second,
            tcp_retransmits,
            tcp_rtt_seconds,
            tcp_rttvar_seconds,
            tcp_snd_cwnd_bytes,
            tcp_snd_wnd_bytes,
            tcp_pmtu_bytes,
            tcp_reorder_events,
            udp_packets,
            udp_lost_packets,
            udp_jitter_seconds,
            udp_out_of_order_packets,
            interval_duration_seconds,
            omitted,
        },
    );
}

fn enqueue_latest(target: &CallbackTarget, metrics: Metrics) {
    match target.tx.try_send(metrics) {
        Ok(()) => {}
        Err(TrySendError::Full(metrics)) => {
            // Prefer freshness over completeness when pushes fall behind.
            if let Some(rx) = &target.latest_rx {
                let _ = rx.try_recv();
            }
            let _ = target.tx.try_send(metrics);
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

/// Aggregate raw interval samples into one representative window.
///
/// Counter-like fields are summed. Gauge-like fields return mean/min/max
/// statistics. Invalid and negative counter values are treated as zero.
pub fn aggregate_window(samples: &[Metrics]) -> Option<WindowMetrics> {
    if samples.is_empty() {
        return None;
    }

    let mut bandwidth = GaugeAccumulator::default();
    let mut tcp_rtt = GaugeAccumulator::default();
    let mut tcp_rttvar = GaugeAccumulator::default();
    let mut tcp_snd_cwnd = GaugeAccumulator::default();
    let mut tcp_snd_wnd = GaugeAccumulator::default();
    let mut tcp_pmtu = GaugeAccumulator::default();
    let mut udp_jitter = GaugeAccumulator::default();

    let mut duration_seconds = 0.0;
    let mut transferred_bytes = 0.0;
    let mut tcp_retransmits = 0.0;
    let mut tcp_reorder_events = 0.0;
    let mut udp_packets = 0.0;
    let mut udp_lost_packets = 0.0;
    let mut udp_out_of_order_packets = 0.0;
    let mut omitted_intervals = 0.0;

    for metrics in samples {
        duration_seconds += finite_nonnegative(metrics.interval_duration_seconds);
        transferred_bytes += finite_nonnegative(metrics.bytes);
        bandwidth.observe(metrics.bandwidth_bits_per_second / 8.0);
        tcp_rtt.observe(metrics.tcp_rtt_seconds);
        tcp_rttvar.observe(metrics.tcp_rttvar_seconds);
        tcp_snd_cwnd.observe(metrics.tcp_snd_cwnd_bytes);
        tcp_snd_wnd.observe(metrics.tcp_snd_wnd_bytes);
        tcp_pmtu.observe(metrics.tcp_pmtu_bytes);
        udp_jitter.observe(metrics.udp_jitter_seconds);
        tcp_retransmits += finite_nonnegative(metrics.tcp_retransmits);
        tcp_reorder_events += finite_nonnegative(metrics.tcp_reorder_events);
        udp_packets += finite_nonnegative(metrics.udp_packets);
        udp_lost_packets += finite_nonnegative(metrics.udp_lost_packets);
        udp_out_of_order_packets += finite_nonnegative(metrics.udp_out_of_order_packets);
        omitted_intervals += finite_nonnegative(metrics.omitted);
    }

    let bandwidth_mean = if duration_seconds > 0.0 {
        transferred_bytes / duration_seconds
    } else {
        bandwidth.finish().mean
    };

    Some(WindowMetrics {
        duration_seconds,
        transferred_bytes,
        bandwidth_bytes_per_second: bandwidth.finish_with_mean(bandwidth_mean),
        tcp_rtt_seconds: tcp_rtt.finish(),
        tcp_rttvar_seconds: tcp_rttvar.finish(),
        tcp_snd_cwnd_bytes: tcp_snd_cwnd.finish(),
        tcp_snd_wnd_bytes: tcp_snd_wnd.finish(),
        tcp_pmtu_bytes: tcp_pmtu.finish(),
        udp_jitter_seconds: udp_jitter.finish(),
        tcp_retransmits,
        tcp_reorder_events,
        udp_packets,
        udp_lost_packets,
        udp_out_of_order_packets,
        omitted_intervals,
    })
}

#[derive(Debug, Clone, Default)]
struct GaugeAccumulator {
    count: usize,
    sum: f64,
    min: f64,
    max: f64,
}

impl GaugeAccumulator {
    fn observe(&mut self, value: f64) {
        if !value.is_finite() {
            return;
        }
        if self.count == 0 {
            self.min = value;
            self.max = value;
        } else {
            self.min = self.min.min(value);
            self.max = self.max.max(value);
        }
        self.count += 1;
        self.sum += value;
    }

    fn finish(&self) -> WindowGaugeStats {
        if self.count == 0 {
            return WindowGaugeStats::default();
        }
        WindowGaugeStats {
            mean: self.sum / self.count as f64,
            min: self.min,
            max: self.max,
        }
    }

    fn finish_with_mean(&self, mean: f64) -> WindowGaugeStats {
        let mut stats = self.finish();
        if self.count > 0 && mean.is_finite() {
            stats.mean = mean;
        }
        stats
    }
}

fn finite_nonnegative(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    // Keep symbolic domains small and concrete enough that Kani explores the
    // aggregation logic itself instead of spending the budget on floating-point
    // arithmetic edge cases already handled by `finite_nonnegative`.
    #[kani::proof]
    fn empty_window_has_no_summary() {
        assert!(aggregate_window(&[]).is_none());
    }

    #[kani::proof]
    fn metrics_mode_callback_policy_matches_variant() {
        let variant: u8 = kani::any();
        let mode = match variant % 3 {
            0 => MetricsMode::Disabled,
            1 => MetricsMode::Interval,
            _ => MetricsMode::Window(Duration::from_secs(1)),
        };

        assert_eq!(mode.is_enabled(), !matches!(mode, MetricsMode::Disabled));
        assert_eq!(mode.callback_queue().is_some(), mode.is_enabled());
    }

    #[kani::proof]
    #[kani::unwind(3)]
    fn window_counters_are_nonnegative_for_bounded_inputs() {
        let sample = Metrics {
            bytes: f64::from(kani::any::<i16>()),
            tcp_retransmits: f64::from(kani::any::<i16>()),
            tcp_reorder_events: f64::from(kani::any::<i16>()),
            udp_packets: f64::from(kani::any::<i16>()),
            udp_lost_packets: f64::from(kani::any::<i16>()),
            udp_out_of_order_packets: f64::from(kani::any::<i16>()),
            interval_duration_seconds: f64::from(kani::any::<i16>()),
            omitted: f64::from(kani::any::<i16>()),
            ..Metrics::default()
        };

        let window = aggregate_window(&[sample]).expect("nonempty windows summarize");

        assert!(window.duration_seconds >= 0.0);
        assert!(window.transferred_bytes >= 0.0);
        assert!(window.tcp_retransmits >= 0.0);
        assert!(window.tcp_reorder_events >= 0.0);
        assert!(window.udp_packets >= 0.0);
        assert!(window.udp_lost_packets >= 0.0);
        assert!(window.udp_out_of_order_packets >= 0.0);
        assert!(window.omitted_intervals >= 0.0);
    }

    #[kani::proof]
    #[kani::unwind(3)]
    fn window_bandwidth_mean_uses_total_bytes_over_duration_for_unit_intervals() {
        let bytes_a: u8 = kani::any();
        let bytes_b: u8 = kani::any();

        let samples = [
            metrics_with_unit_duration(bytes_a),
            metrics_with_unit_duration(bytes_b),
        ];
        let window = aggregate_window(&samples).expect("nonempty windows summarize");

        let expected = (f64::from(bytes_a) + f64::from(bytes_b)) / 2.0;
        assert_eq!(window.bandwidth_bytes_per_second.mean, expected);
    }

    #[kani::proof]
    #[kani::unwind(3)]
    fn window_gauge_statistics_remain_ordered_for_consistent_samples() {
        let bytes_a: u8 = kani::any();
        let bytes_b: u8 = kani::any();
        let rtt_a: u8 = kani::any();
        let rtt_b: u8 = kani::any();

        let samples = [
            Metrics {
                bytes: f64::from(bytes_a),
                bandwidth_bits_per_second: f64::from(bytes_a) * 8.0,
                tcp_rtt_seconds: f64::from(rtt_a),
                interval_duration_seconds: 1.0,
                ..Metrics::default()
            },
            Metrics {
                bytes: f64::from(bytes_b),
                bandwidth_bits_per_second: f64::from(bytes_b) * 8.0,
                tcp_rtt_seconds: f64::from(rtt_b),
                interval_duration_seconds: 1.0,
                ..Metrics::default()
            },
        ];
        let window = aggregate_window(&samples).expect("nonempty windows summarize");

        assert_ordered(window.bandwidth_bytes_per_second);
        assert_ordered(window.tcp_rtt_seconds);
    }

    fn metrics_with_unit_duration(bytes: u8) -> Metrics {
        Metrics {
            bytes: f64::from(bytes),
            bandwidth_bits_per_second: f64::from(bytes) * 8.0,
            interval_duration_seconds: 1.0,
            ..Metrics::default()
        }
    }

    fn assert_ordered(stats: WindowGaugeStats) {
        assert!(stats.min <= stats.mean);
        assert!(stats.mean <= stats.max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_latest_replaces_queued_metric() {
        let (tx, rx) = bounded::<Metrics>(1);
        let target = CallbackTarget {
            tx,
            latest_rx: Some(rx.clone()),
        };

        enqueue_latest(
            &target,
            Metrics {
                bytes: 1.0,
                ..Metrics::default()
            },
        );
        enqueue_latest(
            &target,
            Metrics {
                bytes: 2.0,
                ..Metrics::default()
            },
        );

        assert_eq!(rx.try_recv().unwrap().bytes, 2.0);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn metric_event_stream_forwards_interval_samples() {
        let (tx, rx) = unbounded::<Metrics>();
        let sample = Metrics {
            bytes: 42.0,
            ..Metrics::default()
        };
        let (mut stream, worker) = metric_event_stream(rx, MetricsMode::Interval);

        tx.send(sample.clone()).unwrap();
        drop(tx);

        assert_eq!(stream.next(), Some(MetricEvent::Interval(sample)));
        worker.join().unwrap();
        assert_eq!(stream.next(), None);
    }

    #[test]
    fn metric_event_stream_flushes_final_window() {
        let (tx, rx) = unbounded::<Metrics>();
        let (mut stream, worker) =
            metric_event_stream(rx, MetricsMode::Window(Duration::from_secs(60)));

        tx.send(Metrics {
            bytes: 4.0,
            bandwidth_bits_per_second: 32.0,
            interval_duration_seconds: 1.0,
            ..Metrics::default()
        })
        .unwrap();
        tx.send(Metrics {
            bytes: 8.0,
            bandwidth_bits_per_second: 64.0,
            interval_duration_seconds: 1.0,
            ..Metrics::default()
        })
        .unwrap();
        drop(tx);

        let Some(MetricEvent::Window(window)) = stream.next() else {
            panic!("expected a final window event");
        };
        assert_eq!(window.transferred_bytes, 12.0);
        assert_eq!(window.duration_seconds, 2.0);
        assert_eq!(window.bandwidth_bytes_per_second.mean, 6.0);
        worker.join().unwrap();
        assert_eq!(stream.next(), None);
    }

    #[test]
    fn aggregate_window_returns_none_for_empty_samples() {
        assert!(aggregate_window(&[]).is_none());
    }

    #[test]
    fn aggregate_window_summarizes_interval_samples_by_metric_semantics() {
        let window = aggregate_window(&[
            Metrics {
                bytes: 100.0,
                bandwidth_bits_per_second: 800.0,
                tcp_retransmits: 1.0,
                tcp_rtt_seconds: 0.010,
                tcp_snd_cwnd_bytes: 1000.0,
                udp_packets: 2.0,
                interval_duration_seconds: 1.0,
                ..Metrics::default()
            },
            Metrics {
                bytes: 900.0,
                bandwidth_bits_per_second: 2400.0,
                tcp_retransmits: 2.0,
                tcp_rtt_seconds: 0.030,
                tcp_snd_cwnd_bytes: 3000.0,
                udp_packets: 3.0,
                interval_duration_seconds: 3.0,
                omitted: 1.0,
                ..Metrics::default()
            },
        ])
        .unwrap();

        assert_eq!(window.duration_seconds, 4.0);
        assert_eq!(window.transferred_bytes, 1000.0);
        assert_eq!(
            window.bandwidth_bytes_per_second,
            WindowGaugeStats {
                mean: 250.0,
                min: 100.0,
                max: 300.0
            }
        );
        assert_eq!(
            window.tcp_rtt_seconds,
            WindowGaugeStats {
                mean: 0.020,
                min: 0.010,
                max: 0.030
            }
        );
        assert_eq!(
            window.tcp_snd_cwnd_bytes,
            WindowGaugeStats {
                mean: 2000.0,
                min: 1000.0,
                max: 3000.0
            }
        );
        assert_eq!(window.tcp_retransmits, 3.0);
        assert_eq!(window.udp_packets, 5.0);
        assert_eq!(window.omitted_intervals, 1.0);
    }

    #[test]
    fn aggregate_window_falls_back_to_observed_bandwidth_when_duration_is_zero() {
        let window = aggregate_window(&[
            Metrics {
                bytes: 100.0,
                bandwidth_bits_per_second: 800.0,
                ..Metrics::default()
            },
            Metrics {
                bytes: 900.0,
                bandwidth_bits_per_second: 2400.0,
                ..Metrics::default()
            },
        ])
        .unwrap();

        assert_eq!(window.duration_seconds, 0.0);
        assert_eq!(
            window.bandwidth_bytes_per_second,
            WindowGaugeStats {
                mean: 200.0,
                min: 100.0,
                max: 300.0
            }
        );
    }

    #[test]
    fn aggregate_window_ignores_invalid_counter_values() {
        let window = aggregate_window(&[
            Metrics {
                bytes: f64::NAN,
                bandwidth_bits_per_second: f64::INFINITY,
                tcp_retransmits: -1.0,
                interval_duration_seconds: -1.0,
                ..Metrics::default()
            },
            Metrics {
                bytes: 8.0,
                bandwidth_bits_per_second: 64.0,
                tcp_retransmits: 2.0,
                interval_duration_seconds: 1.0,
                ..Metrics::default()
            },
        ])
        .unwrap();

        assert_eq!(window.duration_seconds, 1.0);
        assert_eq!(window.transferred_bytes, 8.0);
        assert_eq!(window.tcp_retransmits, 2.0);
        assert_eq!(
            window.bandwidth_bytes_per_second,
            WindowGaugeStats {
                mean: 8.0,
                min: 8.0,
                max: 8.0
            }
        );
    }
}
