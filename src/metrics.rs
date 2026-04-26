use std::collections::HashMap;
use std::os::raw::c_double;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};

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
    pub omitted: f64,
}

pub struct IntervalMetricsReporter {
    test_key: usize,
    worker: Option<JoinHandle<()>>,
}

impl IntervalMetricsReporter {
    pub fn attach(test: &mut IperfTest, sink: PushGateway) -> Result<Self> {
        // The C callback must stay quick and nonblocking; a size-one channel is
        // enough because only the newest interval matters for Pushgateway gauges.
        let (tx, rx) = bounded::<Metrics>(1);
        let test_key = test.as_ptr() as usize;
        callbacks()
            .lock()
            .map_err(|_| anyhow!("metrics callback registry is poisoned"))?
            .insert(test_key, CallbackTarget { tx, rx: rx.clone() });

        test.enable_interval_metrics(metrics_callback);

        // Network I/O happens off the libiperf callback path so slow or
        // unavailable Pushgateway writes do not stall the iperf test itself.
        let worker = thread::spawn(move || {
            for metrics in rx {
                if let Err(err) = sink.push(&metrics) {
                    eprintln!("failed to push metrics: {err:#}");
                }
            }
        });

        Ok(Self {
            test_key,
            worker: Some(worker),
        })
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
    rx: Receiver<Metrics>,
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
            omitted,
        },
    );
}

fn enqueue_latest(target: &CallbackTarget, metrics: Metrics) {
    match target.tx.try_send(metrics) {
        Ok(()) => {}
        Err(TrySendError::Full(metrics)) => {
            // Prefer freshness over completeness when pushes fall behind.
            let _ = target.rx.try_recv();
            let _ = target.tx.try_send(metrics);
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_latest_replaces_queued_metric() {
        let (tx, rx) = bounded::<Metrics>(1);
        let target = CallbackTarget { tx, rx: rx.clone() };

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
}
