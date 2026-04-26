use std::collections::HashMap;
use std::os::raw::c_double;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, bounded, unbounded};

use crate::iperf::{IperfTest, RawIperfTest};
use crate::pushgateway::PushGateway;

#[derive(Debug, Clone, Default)]
pub struct Metrics {
    pub bytes: f64,
    pub bandwidth_bits_per_second: f64,
    pub tcp_retransmits: f64,
    pub tcp_rtt_seconds: f64,
    pub tcp_rttvar_seconds: f64,
    pub tcp_snd_cwnd_bytes: f64,
    pub tcp_snd_wnd_bytes: f64,
    pub tcp_pmtu_bytes: f64,
    pub tcp_reorder_events: f64,
    pub udp_packets: f64,
    pub udp_lost_packets: f64,
    pub udp_jitter_seconds: f64,
    pub udp_out_of_order_packets: f64,
    pub interval_duration_seconds: f64,
    pub omitted: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WindowGaugeStats {
    pub mean: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowMetrics {
    pub duration_seconds: f64,
    pub transferred_bytes: f64,
    pub bandwidth_bytes_per_second: WindowGaugeStats,
    pub tcp_rtt_seconds: WindowGaugeStats,
    pub tcp_rttvar_seconds: WindowGaugeStats,
    pub tcp_snd_cwnd_bytes: WindowGaugeStats,
    pub tcp_snd_wnd_bytes: WindowGaugeStats,
    pub tcp_pmtu_bytes: WindowGaugeStats,
    pub udp_jitter_seconds: WindowGaugeStats,
    pub tcp_retransmits: f64,
    pub tcp_reorder_events: f64,
    pub udp_packets: f64,
    pub udp_lost_packets: f64,
    pub udp_out_of_order_packets: f64,
    pub omitted_intervals: f64,
}

pub struct IntervalMetricsReporter {
    test_key: usize,
    worker: Option<JoinHandle<()>>,
}

impl IntervalMetricsReporter {
    pub fn attach(
        test: &mut IperfTest,
        sink: PushGateway,
        push_interval: Option<Duration>,
    ) -> Result<Self> {
        let (target, rx) = callback_channel(push_interval);
        let test_key = test.as_ptr() as usize;
        callbacks()
            .lock()
            .map_err(|_| anyhow!("metrics callback registry is poisoned"))?
            .insert(test_key, target);

        test.enable_interval_metrics(metrics_callback);

        // Network I/O happens off the libiperf callback path so slow or
        // unavailable Pushgateway writes do not stall the iperf test itself.
        let worker = thread::spawn(move || match push_interval {
            Some(interval) => push_window_metrics(rx, sink, interval),
            None => push_interval_metrics(rx, sink),
        });

        Ok(Self {
            test_key,
            worker: Some(worker),
        })
    }
}

fn callback_channel(push_interval: Option<Duration>) -> (CallbackTarget, Receiver<Metrics>) {
    if push_interval.is_some() {
        // Window aggregation needs every libiperf interval sample in the window.
        // The worker owns flushing, so use an unbounded channel rather than the
        // freshness-only replacement queue used for immediate gauges.
        let (tx, rx) = unbounded::<Metrics>();
        (
            CallbackTarget {
                tx,
                latest_rx: None,
            },
            rx,
        )
    } else {
        // Without window aggregation, Pushgateway stores only the latest value
        // for a grouping key. Keep the callback nonblocking and replace stale
        // queued samples if HTTP writes fall behind.
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
        if let Ok(mut callbacks) = callbacks().lock() {
            callbacks.remove(&self.test_key);
        }
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
