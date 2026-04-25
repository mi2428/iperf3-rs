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

