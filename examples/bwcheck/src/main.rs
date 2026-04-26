//! Example application for using `iperf3-rs` as a library crate.
//!
//! This binary is intentionally small but complete: it accepts one or more
//! `HOST:PORT` endpoints, runs a fixed UDP iperf3 test against each endpoint,
//! consumes live interval metrics through the Rust API, and exits nonzero when
//! any endpoint falls below the requested bandwidth/loss thresholds. It is used
//! both as documentation and as an integration-test target for the public
//! library surface.

use std::env;
use std::fmt;
use std::process::ExitCode;
use std::time::Duration;

use iperf3_rs::{IperfCommand, MetricEvent, Metrics, MetricsMode};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// The thresholds are application policy, not iperf policy. A real application
// could source these from config files, service metadata, or a monitoring rule.
const DEFAULT_MIN_BANDWIDTH_BPS: f64 = 500_000.0;
const DEFAULT_MAX_LOSS_PERCENT: f64 = 1.0;

// The example fixes the iperf parameters so that the library usage is the main
// thing to read. The typed helpers below are thin wrappers over normal iperf
// options; `arg` and `args` remain available for less common upstream flags.
const IPERF_SECONDS: u64 = 3;
const IPERF_INTERVAL_SECONDS: u64 = 1;
const IPERF_UDP_BITRATE_BPS: u64 = 1_000_000;

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("{err}");
            eprintln!();
            print_usage();
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool> {
    let Some(config) = parse_args(env::args())? else {
        print_usage();
        return Ok(true);
    };

    // `IperfCommand` serializes high-level runs inside this process because
    // libiperf owns some process-global state. Running endpoints sequentially is
    // therefore the expected shape for this first library API.
    let mut failed = 0usize;
    for endpoint in &config.endpoints {
        let report = check_endpoint(endpoint, &config)?;
        if report.passed {
            println!("{report}");
        } else {
            failed += 1;
            println!("{report}");
        }
    }

    println!("summary checked={} failed={failed}", config.endpoints.len());
    Ok(failed == 0)
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Config>> {
    // This example parses only its own application options. The actual iperf
    // options used for traffic generation remain argv-shaped and are passed to
    // libiperf later through `IperfCommand::args`.
    let mut min_bandwidth_bps = DEFAULT_MIN_BANDWIDTH_BPS;
    let mut max_loss_percent = DEFAULT_MAX_LOSS_PERCENT;
    let mut endpoints = Vec::new();
    let mut args = args.into_iter().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--min-bandwidth-bps" => {
                let value = args.next().ok_or("--min-bandwidth-bps requires a value")?;
                min_bandwidth_bps = parse_nonnegative_f64(&value, "--min-bandwidth-bps")?;
            }
            "--max-loss-percent" => {
                let value = args.next().ok_or("--max-loss-percent requires a value")?;
                max_loss_percent = parse_nonnegative_f64(&value, "--max-loss-percent")?;
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}").into()),
            _ => endpoints.push(Endpoint::parse(&arg)?),
        }
    }

    if endpoints.is_empty() {
        return Err("at least one HOST:PORT endpoint is required".into());
    }

    Ok(Some(Config {
        min_bandwidth_bps,
        max_loss_percent,
        endpoints,
    }))
}

fn parse_nonnegative_f64(raw: &str, name: &str) -> Result<f64> {
    let value = raw
        .parse::<f64>()
        .map_err(|err| format!("{name} must be a number: {err}"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{name} must be a finite non-negative number").into());
    }
    Ok(value)
}

#[derive(Debug)]
struct Config {
    min_bandwidth_bps: f64,
    max_loss_percent: f64,
    endpoints: Vec<Endpoint>,
}

#[derive(Debug)]
struct Endpoint {
    raw: String,
    host: String,
    port: u16,
}

impl Endpoint {
    fn parse(raw: &str) -> Result<Self> {
        // Split from the right so hostnames containing ':' can be handled later
        // without changing the call site. This sample still expects a plain
        // `HOST:PORT` value and validates only enough for the demonstration.
        let (host, port) = raw
            .rsplit_once(':')
            .ok_or_else(|| format!("endpoint must be HOST:PORT: {raw}"))?;
        if host.is_empty() {
            return Err(format!("endpoint host must not be empty: {raw}").into());
        }
        let port = port
            .parse::<u16>()
            .map_err(|err| format!("endpoint port must be a TCP/UDP port: {raw}: {err}"))?;
        if port == 0 {
            return Err(format!("endpoint port must not be zero: {raw}").into());
        }

        Ok(Self {
            raw: raw.to_owned(),
            host: host.to_owned(),
            port,
        })
    }
}

#[derive(Debug)]
struct CheckReport {
    endpoint: String,
    bandwidth_bps: f64,
    loss_percent: f64,
    packets: f64,
    lost_packets: f64,
    passed: bool,
}

impl fmt::Display for CheckReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = if self.passed { "PASS" } else { "FAIL" };
        write!(
            f,
            "{status} endpoint={} bandwidth_bps={:.0} loss_percent={:.3} packets={:.0} lost_packets={:.0}",
            self.endpoint, self.bandwidth_bps, self.loss_percent, self.packets, self.lost_packets
        )
    }
}

fn check_endpoint(endpoint: &Endpoint, config: &Config) -> Result<CheckReport> {
    let mut command = IperfCommand::client(endpoint.host.as_str());
    // The typed builder still produces ordinary iperf arguments, then upstream
    // libiperf parses them. The Rust application owns orchestration around
    // libiperf: which endpoints to run, which thresholds matter, and what to do
    // with the live metrics.
    command
        .port(endpoint.port)
        .udp()
        .bitrate_bits_per_second(IPERF_UDP_BITRATE_BPS)
        .duration(Duration::from_secs(IPERF_SECONDS))
        .report_interval(Duration::from_secs(IPERF_INTERVAL_SECONDS));

    // `spawn_with_metrics` starts the iperf run and returns immediately with
    // both the process-local runner and the metrics stream. In interval mode,
    // each libiperf report interval becomes a `MetricEvent::Interval` while the
    // iperf run is still active.
    let (running, stream) = command.spawn_with_metrics(MetricsMode::Interval)?;
    let mut samples = Vec::new();

    // Drain the stream until the producer closes it. This is the same pattern a
    // bot or service would use if it wanted to make live decisions during a
    // test instead of waiting for final JSON output.
    while let Some(event) = stream.recv() {
        if let MetricEvent::Interval(metrics) = event {
            samples.push(metrics);
        }
    }

    // Always wait for the run result after consuming metrics. `wait` propagates
    // libiperf failures such as unreachable servers or invalid iperf arguments.
    running.wait()?;

    let summary = summarize_samples(&samples)?;
    let passed = summary.bandwidth_bps >= config.min_bandwidth_bps
        && summary.loss_percent < config.max_loss_percent;

    Ok(CheckReport {
        endpoint: endpoint.raw.clone(),
        bandwidth_bps: summary.bandwidth_bps,
        loss_percent: summary.loss_percent,
        packets: summary.packets,
        lost_packets: summary.lost_packets,
        passed,
    })
}

#[derive(Debug)]
struct MetricsSummary {
    bandwidth_bps: f64,
    loss_percent: f64,
    packets: f64,
    lost_packets: f64,
}

fn summarize_samples(samples: &[Metrics]) -> Result<MetricsSummary> {
    // For this example, bandwidth and loss are computed from the raw interval
    // counters exposed by the library. This keeps the application decision close
    // to the values libiperf observed, without scraping terminal output.
    let mut bytes = 0.0;
    let mut seconds = 0.0;
    let mut packets = 0.0;
    let mut lost_packets = 0.0;

    // Omitted intervals are warm-up intervals excluded by iperf. They should not
    // affect an application-level pass/fail decision.
    for sample in samples.iter().filter(|sample| !sample.omitted) {
        bytes += finite_nonnegative(sample.transferred_bytes);
        seconds += finite_nonnegative(sample.interval_duration_seconds);
        packets += finite_nonnegative(sample.udp_packets.unwrap_or(0.0));
        lost_packets += finite_nonnegative(sample.udp_lost_packets.unwrap_or(0.0));
    }

    if seconds == 0.0 {
        return Err("iperf run produced no non-omitted interval duration".into());
    }
    // UDP packet counters are the reason this example uses UDP. They let the
    // application compute loss directly from libiperf metrics.
    if packets + lost_packets == 0.0 {
        return Err("iperf run produced no UDP packet counters".into());
    }

    Ok(MetricsSummary {
        bandwidth_bps: (bytes * 8.0) / seconds,
        loss_percent: (lost_packets / (packets + lost_packets)) * 100.0,
        packets,
        lost_packets,
    })
}

fn finite_nonnegative(value: f64) -> f64 {
    // Treat bad metric values as absent samples. The public API should normally
    // expose finite values, but examples that aggregate monitoring data should
    // still be defensive at the edge.
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

fn print_usage() {
    eprintln!(
        "usage: iperf3-rs-bwcheck [--min-bandwidth-bps N] [--max-loss-percent N] HOST:PORT..."
    );
    eprintln!(
        "fixed iperf parameters: -u -b {IPERF_UDP_BITRATE_BPS} -t {IPERF_SECONDS} -i {IPERF_INTERVAL_SECONDS}"
    );
}
