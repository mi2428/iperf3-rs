use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use serde::Deserialize;

use crate::iperf::{IperfTest, RawIperfTest};
use crate::pushgateway::PushGateway;

#[derive(Debug, Clone, Default)]
pub struct Metrics {
    pub bytes: f64,
    pub bandwidth_bits_per_second: f64,
    pub packets: f64,
    pub error_packets: f64,
    pub jitter_seconds: f64,
    pub tcp_retransmits: f64,
}

pub struct JsonMetricsReporter {
    test_key: usize,
    worker: Option<JoinHandle<()>>,
}

impl JsonMetricsReporter {
    pub fn attach(test: &mut IperfTest, sink: PushGateway, mirror_json: bool) -> Result<Self> {
        // The C callback must stay quick and nonblocking; a size-one channel is
        // enough because only the newest interval matters for Pushgateway gauges.
        let (tx, rx) = bounded::<String>(1);
        let test_key = test.as_ptr() as usize;
        callbacks()
            .lock()
            .map_err(|_| anyhow!("json callback registry is poisoned"))?
            .insert(
                test_key,
                CallbackTarget {
                    tx,
                    rx: rx.clone(),
                    mirror_json,
                },
            );

        test.enable_json_metrics(json_callback);

        // Network I/O happens off the libiperf callback path so slow or
        // unavailable Pushgateway writes do not stall the iperf test itself.
        let worker = thread::spawn(move || {
            for line in rx {
                let Some(metrics) = metrics_from_json_line(&line) else {
                    continue;
                };
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

impl Drop for JsonMetricsReporter {
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
    tx: Sender<String>,
    rx: Receiver<String>,
    mirror_json: bool,
}

static CALLBACKS: OnceLock<Mutex<HashMap<usize, CallbackTarget>>> = OnceLock::new();

fn callbacks() -> &'static Mutex<HashMap<usize, CallbackTarget>> {
    // The same extern callback is registered for every test, so dispatch by the
    // iperf_test pointer passed back from C.
    CALLBACKS.get_or_init(|| Mutex::new(HashMap::new()))
}

unsafe extern "C" fn json_callback(test: *mut RawIperfTest, json: *mut c_char) {
    if test.is_null() || json.is_null() {
        return;
    }

    let line = unsafe { CStr::from_ptr(json) }
        .to_string_lossy()
        .into_owned();
    let Ok(callbacks) = callbacks().lock() else {
        return;
    };
    let Some(target) = callbacks.get(&(test as usize)) else {
        return;
    };

    if target.mirror_json {
        println!("{line}");
    }

    enqueue_latest(target, line);
}

fn enqueue_latest(target: &CallbackTarget, line: String) {
    match target.tx.try_send(line) {
        Ok(()) => {}
        Err(TrySendError::Full(line)) => {
            // Prefer freshness over completeness when pushes fall behind.
            let _ = target.rx.try_recv();
            let _ = target.tx.try_send(line);
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

fn metrics_from_json_line(line: &str) -> Option<Metrics> {
    let event: JsonStreamEvent = serde_json::from_str(line).ok()?;
    if event.event != "interval" {
        return None;
    }

    // Normal TCP/UDP reports use `sum`; bidirectional reverse reports use a
    // separate aggregate, and older/edge JSON shapes may only have streams.
    let sum = event
        .data
        .sum
        .or(event.data.sum_bidir_reverse)
        .or_else(|| event.data.streams.into_iter().next())?;

    Some(Metrics {
        bytes: sum.bytes.unwrap_or_default(),
        bandwidth_bits_per_second: sum.bits_per_second.unwrap_or_default(),
        packets: sum.packets.unwrap_or_default(),
        error_packets: sum.lost_packets.unwrap_or_default(),
        jitter_seconds: sum.jitter_ms.unwrap_or_default() / 1000.0,
        tcp_retransmits: sum.retransmits.unwrap_or_default(),
    })
}

#[derive(Debug, Deserialize)]
struct JsonStreamEvent {
    event: String,
    data: IntervalData,
}

#[derive(Debug, Deserialize)]
struct IntervalData {
    #[serde(default)]
    streams: Vec<IntervalSum>,
    sum: Option<IntervalSum>,
    sum_bidir_reverse: Option<IntervalSum>,
}

#[derive(Debug, Deserialize)]
struct IntervalSum {
    bytes: Option<f64>,
    bits_per_second: Option<f64>,
    packets: Option<f64>,
    lost_packets: Option<f64>,
    jitter_ms: Option<f64>,
    retransmits: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tcp_interval_sum() {
        let metrics = metrics_from_json_line(
            r#"{"event":"interval","data":{"streams":[],"sum":{"bytes":1000,"bits_per_second":8000,"retransmits":2}}}"#,
        )
        .unwrap();

        assert_eq!(metrics.bytes, 1000.0);
        assert_eq!(metrics.bandwidth_bits_per_second, 8000.0);
        assert_eq!(metrics.tcp_retransmits, 2.0);
    }

    #[test]
    fn parses_udp_interval_sum() {
        let metrics = metrics_from_json_line(
            r#"{"event":"interval","data":{"streams":[],"sum":{"bytes":1000,"bits_per_second":8000,"packets":10,"lost_packets":1,"jitter_ms":5.5}}}"#,
        )
        .unwrap();

        assert_eq!(metrics.packets, 10.0);
        assert_eq!(metrics.error_packets, 1.0);
        assert_eq!(metrics.jitter_seconds, 0.0055);
    }

    #[test]
    fn ignores_non_interval_events() {
        assert!(metrics_from_json_line(r#"{"event":"start","data":{"streams":[]}}"#).is_none());
        assert!(metrics_from_json_line("not-json").is_none());
    }

    #[test]
    fn falls_back_to_bidir_reverse_sum() {
        let metrics = metrics_from_json_line(
            r#"{"event":"interval","data":{"streams":[],"sum_bidir_reverse":{"bytes":42,"bits_per_second":336}}}"#,
        )
        .unwrap();

        assert_eq!(metrics.bytes, 42.0);
        assert_eq!(metrics.bandwidth_bits_per_second, 336.0);
    }

    #[test]
    fn falls_back_to_first_stream_when_sum_is_absent() {
        let metrics = metrics_from_json_line(
            r#"{"event":"interval","data":{"streams":[{"bytes":7,"bits_per_second":56,"retransmits":1}]}}"#,
        )
        .unwrap();

        assert_eq!(metrics.bytes, 7.0);
        assert_eq!(metrics.bandwidth_bits_per_second, 56.0);
        assert_eq!(metrics.tcp_retransmits, 1.0);
    }

    #[test]
    fn enqueue_latest_replaces_queued_metric() {
        let (tx, rx) = bounded::<String>(1);
        let target = CallbackTarget {
            tx,
            rx: rx.clone(),
            mirror_json: false,
        };

        enqueue_latest(&target, "old".to_owned());
        enqueue_latest(&target, "new".to_owned());

        assert_eq!(rx.try_recv().unwrap(), "new");
        assert!(rx.try_recv().is_err());
    }
}
