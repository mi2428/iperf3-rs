//! Pushgateway rendering and HTTP delivery helpers.
//!
//! The CLI uses these types internally, and library users can also construct a
//! [`PushGateway`] when they want to push [`crate::Metrics`] or
//! [`crate::WindowMetrics`] collected from [`crate::MetricsStream`].

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::Client;
use url::Url;

use crate::metrics::{Metrics, WindowGaugeStats, WindowMetrics};
use crate::{Error, ErrorKind, Result};

const PUSH_RETRY_BASE_DELAY: Duration = Duration::from_millis(100);
const PUSH_RETRY_MAX_DELAY: Duration = Duration::from_secs(1);

/// Configuration for a [`PushGateway`] sink.
#[derive(Debug, Clone)]
pub struct PushGatewayConfig {
    /// Base Pushgateway URL.
    ///
    /// The final request path is built as
    /// `/metrics/job/{job}/{label}/{value}/...`.
    pub endpoint: Url,
    /// Pushgateway job name.
    pub job: String,
    /// Grouping labels encoded into the Pushgateway request path.
    pub labels: Vec<(String, String)>,
    /// Per-request HTTP timeout.
    pub timeout: Duration,
    /// Number of retries after the first failed request.
    pub retries: u32,
    /// HTTP `User-Agent` header.
    pub user_agent: String,
    /// Prefix for emitted Prometheus metric names.
    pub metric_prefix: String,
}

/// HTTP sink for pushing iperf metrics to Prometheus Pushgateway.
pub struct PushGateway {
    client: Client,
    url: Url,
    retries: u32,
    metric_prefix: String,
}

impl PushGateway {
    /// Build a Pushgateway sink from validated configuration.
    pub fn new(config: PushGatewayConfig) -> Result<Self> {
        let mut url = config.endpoint;
        let mut path = url.path().trim_end_matches('/').to_owned();
        // Pushgateway represents grouping labels as path segments:
        // /metrics/job/<job>/<label>/<value>/...
        path.push_str("/metrics/job/");
        path.push_str(&encode_path_segment(&config.job));
        for (name, value) in config.labels {
            path.push('/');
            path.push_str(&encode_path_segment(&name));
            path.push('/');
            path.push_str(&encode_path_segment(&value));
        }
        url.set_path(&path);

        let client = Client::builder()
            // Metrics are best-effort; a stuck gateway should not hold the iperf
            // process indefinitely.
            .timeout(config.timeout)
            .user_agent(config.user_agent)
            .build()
            .map_err(|err| {
                Error::with_source(ErrorKind::PushGateway, "failed to build HTTP client", err)
            })?;

        Ok(Self {
            client,
            url,
            retries: config.retries,
            metric_prefix: config.metric_prefix,
        })
    }

    /// Push one immediate interval sample.
    pub fn push(&self, metrics: &Metrics) -> Result<()> {
        let body = render_prometheus(metrics, &self.metric_prefix);
        self.push_body(&body)
    }

    /// Push one aggregated window summary.
    pub fn push_window(&self, metrics: &WindowMetrics) -> Result<()> {
        let body = render_window_prometheus(metrics, &self.metric_prefix);
        self.push_body(&body)
    }

    fn push_body(&self, body: &str) -> Result<()> {
        for attempt in 0..=self.retries {
            match self.push_once(body) {
                Ok(()) => return Ok(()),
                Err(err) if err.retryable && attempt < self.retries => {
                    std::thread::sleep(retry_delay(attempt));
                }
                Err(err) => return Err(err.error),
            }
        }

        unreachable!("push retry loop always returns")
    }

    fn push_once(&self, body: &str) -> std::result::Result<(), PushAttemptError> {
        let response = self
            .client
            .put(self.url.clone())
            .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
            .body(body.to_owned())
            .send()
            .map_err(|err| PushAttemptError {
                error: Error::with_source(
                    ErrorKind::PushGateway,
                    "failed to send Pushgateway request",
                    err,
                ),
                retryable: true,
            })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(PushAttemptError {
                error: Error::pushgateway(format!("Pushgateway returned {status}")),
                retryable: is_retryable_status(status),
            });
        }

        Ok(())
    }
}

#[derive(Debug)]
struct PushAttemptError {
    error: Error,
    retryable: bool,
}

fn is_retryable_status(status: StatusCode) -> bool {
    is_retryable_status_code(status.as_u16())
}

fn is_retryable_status_code(status: u16) -> bool {
    (500..=599).contains(&status) || status == 429
}

fn retry_delay(attempt: u32) -> Duration {
    PUSH_RETRY_BASE_DELAY
        .saturating_mul(2_u32.saturating_pow(attempt))
        .min(PUSH_RETRY_MAX_DELAY)
}

fn render_prometheus(metrics: &Metrics, prefix: &str) -> String {
    let mut out = String::new();
    gauge(&mut out, &metric_name(prefix, "bytes"), metrics.bytes);
    gauge(
        &mut out,
        &metric_name(prefix, "bandwidth"),
        metrics.bandwidth_bits_per_second,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_retransmits"),
        metrics.tcp_retransmits,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_rtt_seconds"),
        metrics.tcp_rtt_seconds,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_rttvar_seconds"),
        metrics.tcp_rttvar_seconds,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_snd_cwnd_bytes"),
        metrics.tcp_snd_cwnd_bytes,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_snd_wnd_bytes"),
        metrics.tcp_snd_wnd_bytes,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_pmtu_bytes"),
        metrics.tcp_pmtu_bytes,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_reorder_events"),
        metrics.tcp_reorder_events,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "udp_packets"),
        metrics.udp_packets,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "udp_lost_packets"),
        metrics.udp_lost_packets,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "udp_jitter_seconds"),
        metrics.udp_jitter_seconds,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "udp_out_of_order_packets"),
        metrics.udp_out_of_order_packets,
    );
    gauge(&mut out, &metric_name(prefix, "omitted"), metrics.omitted);
    out
}

fn render_window_prometheus(metrics: &WindowMetrics, prefix: &str) -> String {
    let mut out = String::new();
    gauge(
        &mut out,
        &metric_name(prefix, "window_duration_seconds"),
        metrics.duration_seconds,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "window_transferred_bytes"),
        metrics.transferred_bytes,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_bandwidth",
        "bytes_per_second",
        metrics.bandwidth_bytes_per_second,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_tcp_rtt",
        "seconds",
        metrics.tcp_rtt_seconds,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_tcp_rttvar",
        "seconds",
        metrics.tcp_rttvar_seconds,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_tcp_snd_cwnd",
        "bytes",
        metrics.tcp_snd_cwnd_bytes,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_tcp_snd_wnd",
        "bytes",
        metrics.tcp_snd_wnd_bytes,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_tcp_pmtu",
        "bytes",
        metrics.tcp_pmtu_bytes,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_udp_jitter",
        "seconds",
        metrics.udp_jitter_seconds,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "window_tcp_retransmits"),
        metrics.tcp_retransmits,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "window_tcp_reorder_events"),
        metrics.tcp_reorder_events,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "window_udp_packets"),
        metrics.udp_packets,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "window_udp_lost_packets"),
        metrics.udp_lost_packets,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "window_udp_out_of_order_packets"),
        metrics.udp_out_of_order_packets,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "window_omitted_intervals"),
        metrics.omitted_intervals,
    );
    out
}

fn metric_name(prefix: &str, suffix: &str) -> String {
    format!("{prefix}_{suffix}")
}

fn gauge_stats(out: &mut String, prefix: &str, stem: &str, unit: &str, stats: WindowGaugeStats) {
    if stats.samples == 0 {
        return;
    }
    gauge(
        out,
        &metric_name(prefix, &format!("{stem}_mean_{unit}")),
        stats.mean,
    );
    gauge(
        out,
        &metric_name(prefix, &format!("{stem}_min_{unit}")),
        stats.min,
    );
    gauge(
        out,
        &metric_name(prefix, &format!("{stem}_max_{unit}")),
        stats.max,
    );
}

fn gauge(out: &mut String, name: &str, value: f64) {
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push_str(" gauge\n");
    out.push_str(name);
    out.push(' ');
    out.push_str(&value.to_string());
    out.push('\n');
}

fn gauge_option(out: &mut String, name: &str, value: Option<f64>) {
    if let Some(value) = value {
        gauge(out, name, value);
    }
}

fn encode_path_segment(raw: &str) -> String {
    // Path segments cannot be delegated to Url::path_segments_mut here because
    // the Pushgateway grouping path is assembled onto any existing base path.
    let mut encoded = String::new();
    for byte in raw.bytes() {
        let encoded_byte = encode_path_byte(byte);
        for &byte in &encoded_byte.bytes[..encoded_byte.len] {
            encoded.push(byte as char);
        }
    }
    encoded
}

#[derive(Debug, Clone, Copy)]
struct EncodedPathByte {
    bytes: [u8; 3],
    len: usize,
}

fn encode_path_byte(byte: u8) -> EncodedPathByte {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    if is_unreserved_path_byte(byte) {
        return EncodedPathByte {
            bytes: [byte, 0, 0],
            len: 1,
        };
    }

    EncodedPathByte {
        bytes: [b'%', HEX[(byte >> 4) as usize], HEX[(byte & 0x0f) as usize]],
        len: 3,
    }
}

fn is_unreserved_path_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~'
    )
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    #[kani::unwind(4)]
    fn path_byte_encoding_escapes_reserved_bytes() {
        let byte: u8 = kani::any();
        let encoded = encode_path_byte(byte);

        if is_unreserved_path_byte(byte) {
            assert_eq!(encoded.len, 1);
            assert_eq!(encoded.bytes[0], byte);
        } else {
            assert_eq!(encoded.len, 3);
            assert_eq!(encoded.bytes[0], b'%');
            assert!(encoded.bytes[1].is_ascii_hexdigit());
            assert!(encoded.bytes[2].is_ascii_hexdigit());
        }

        for i in 0..encoded.len {
            assert_ne!(encoded.bytes[i], b'/');
            assert_ne!(encoded.bytes[i], b' ');
        }
    }

    #[kani::proof]
    fn retryable_status_codes_match_pushgateway_retry_policy() {
        let status: u16 = kani::any();
        let expected = status == 429 || (500..=599).contains(&status);

        assert_eq!(is_retryable_status_code(status), expected);
    }

    #[kani::proof]
    #[kani::unwind(12)]
    fn retry_delay_is_bounded_for_configured_retry_counts() {
        let attempt: u32 = kani::any();
        kani::assume(attempt <= 10);

        let delay = retry_delay(attempt);

        assert!(delay >= PUSH_RETRY_BASE_DELAY);
        assert!(delay <= PUSH_RETRY_MAX_DELAY);
        assert!(!delay.is_zero());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_grouping_segments() {
        assert_eq!(encode_path_segment("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn renders_prometheus_gauges() {
        let rendered = render_prometheus(
            &Metrics {
                bytes: 1.0,
                bandwidth_bits_per_second: 8.0,
                tcp_retransmits: Some(5.0),
                tcp_rtt_seconds: Some(0.006),
                tcp_rttvar_seconds: Some(0.007),
                tcp_snd_cwnd_bytes: Some(8.0),
                tcp_snd_wnd_bytes: Some(9.0),
                tcp_pmtu_bytes: Some(10.0),
                tcp_reorder_events: Some(11.0),
                udp_packets: Some(2.0),
                udp_lost_packets: Some(3.0),
                udp_jitter_seconds: Some(0.004),
                udp_out_of_order_packets: Some(12.0),
                interval_duration_seconds: 1.0,
                omitted: 1.0,
                ..Metrics::default()
            },
            "iperf3",
        );
        assert!(rendered.contains("iperf3_bytes 1\n"));
        assert!(rendered.contains("iperf3_tcp_rtt_seconds 0.006\n"));
        assert!(rendered.contains("iperf3_udp_packets 2\n"));
        assert!(rendered.contains("iperf3_udp_lost_packets 3\n"));
        assert!(rendered.contains("iperf3_udp_jitter_seconds 0.004\n"));
        assert!(rendered.contains("iperf3_udp_out_of_order_packets 12\n"));
        assert!(rendered.contains("iperf3_omitted 1\n"));
    }

    #[test]
    fn builds_pushgateway_grouping_url() {
        let gateway = PushGateway::new(PushGatewayConfig {
            endpoint: Url::parse("http://127.0.0.1:9091/base/").unwrap(),
            job: "iperf job".to_owned(),
            labels: vec![
                ("test".to_owned(), "test/one".to_owned()),
                ("scenario".to_owned(), "sample#1".to_owned()),
                ("mode".to_owned(), "client".to_owned()),
            ],
            timeout: Duration::from_secs(5),
            retries: 0,
            user_agent: "iperf3-rs/test".to_owned(),
            metric_prefix: "iperf3".to_owned(),
        })
        .unwrap();

        assert_eq!(
            gateway.url.as_str(),
            "http://127.0.0.1:9091/base/metrics/job/iperf%20job/test/test%2Fone/scenario/sample%231/mode/client"
        );
    }

    #[test]
    fn renders_all_expected_metric_names() {
        let rendered = render_prometheus(&Metrics::default(), "iperf3");

        for name in ["iperf3_bytes", "iperf3_bandwidth", "iperf3_omitted"] {
            assert!(rendered.contains(&format!("# TYPE {name} gauge\n")));
            assert!(rendered.contains(&format!("{name} 0\n")));
        }

        for name in [
            "iperf3_tcp_retransmits",
            "iperf3_tcp_rtt_seconds",
            "iperf3_tcp_rttvar_seconds",
            "iperf3_tcp_snd_cwnd_bytes",
            "iperf3_tcp_snd_wnd_bytes",
            "iperf3_tcp_pmtu_bytes",
            "iperf3_tcp_reorder_events",
            "iperf3_udp_packets",
            "iperf3_udp_lost_packets",
            "iperf3_udp_jitter_seconds",
            "iperf3_udp_out_of_order_packets",
        ] {
            assert!(!rendered.contains(&format!("# TYPE {name} gauge\n")));
        }
    }

    #[test]
    fn renders_window_prometheus_gauges() {
        let rendered = render_window_prometheus(
            &WindowMetrics {
                duration_seconds: 10.0,
                transferred_bytes: 1000.0,
                bandwidth_bytes_per_second: WindowGaugeStats {
                    samples: 2,
                    mean: 100.0,
                    min: 90.0,
                    max: 110.0,
                },
                tcp_rtt_seconds: WindowGaugeStats {
                    samples: 2,
                    mean: 0.010,
                    min: 0.005,
                    max: 0.020,
                },
                tcp_retransmits: Some(3.0),
                udp_packets: Some(4.0),
                udp_lost_packets: Some(1.0),
                omitted_intervals: 2.0,
                ..WindowMetrics::default()
            },
            "iperf3",
        );

        assert!(rendered.contains("iperf3_window_duration_seconds 10\n"));
        assert!(rendered.contains("iperf3_window_transferred_bytes 1000\n"));
        assert!(rendered.contains("iperf3_window_bandwidth_mean_bytes_per_second 100\n"));
        assert!(rendered.contains("iperf3_window_bandwidth_min_bytes_per_second 90\n"));
        assert!(rendered.contains("iperf3_window_bandwidth_max_bytes_per_second 110\n"));
        assert!(rendered.contains("iperf3_window_tcp_rtt_mean_seconds 0.01\n"));
        assert!(rendered.contains("iperf3_window_tcp_rtt_min_seconds 0.005\n"));
        assert!(rendered.contains("iperf3_window_tcp_rtt_max_seconds 0.02\n"));
        assert!(rendered.contains("iperf3_window_tcp_retransmits 3\n"));
        assert!(rendered.contains("iperf3_window_udp_packets 4\n"));
        assert!(rendered.contains("iperf3_window_udp_lost_packets 1\n"));
        assert!(rendered.contains("iperf3_window_omitted_intervals 2\n"));
    }

    #[test]
    fn renders_all_expected_window_metric_names() {
        let rendered = render_window_prometheus(&WindowMetrics::default(), "iperf3");

        for name in [
            "iperf3_window_duration_seconds",
            "iperf3_window_transferred_bytes",
            "iperf3_window_omitted_intervals",
        ] {
            assert!(rendered.contains(&format!("# TYPE {name} gauge\n")));
            assert!(rendered.contains(&format!("{name} 0\n")));
        }

        for name in [
            "iperf3_window_bandwidth_mean_bytes_per_second",
            "iperf3_window_bandwidth_min_bytes_per_second",
            "iperf3_window_bandwidth_max_bytes_per_second",
            "iperf3_window_tcp_rtt_mean_seconds",
            "iperf3_window_tcp_rtt_min_seconds",
            "iperf3_window_tcp_rtt_max_seconds",
            "iperf3_window_tcp_rttvar_mean_seconds",
            "iperf3_window_tcp_rttvar_min_seconds",
            "iperf3_window_tcp_rttvar_max_seconds",
            "iperf3_window_tcp_snd_cwnd_mean_bytes",
            "iperf3_window_tcp_snd_cwnd_min_bytes",
            "iperf3_window_tcp_snd_cwnd_max_bytes",
            "iperf3_window_tcp_snd_wnd_mean_bytes",
            "iperf3_window_tcp_snd_wnd_min_bytes",
            "iperf3_window_tcp_snd_wnd_max_bytes",
            "iperf3_window_tcp_pmtu_mean_bytes",
            "iperf3_window_tcp_pmtu_min_bytes",
            "iperf3_window_tcp_pmtu_max_bytes",
            "iperf3_window_udp_jitter_mean_seconds",
            "iperf3_window_udp_jitter_min_seconds",
            "iperf3_window_udp_jitter_max_seconds",
            "iperf3_window_tcp_retransmits",
            "iperf3_window_tcp_reorder_events",
            "iperf3_window_udp_packets",
            "iperf3_window_udp_lost_packets",
            "iperf3_window_udp_out_of_order_packets",
        ] {
            assert!(!rendered.contains(&format!("# TYPE {name} gauge\n")));
        }
    }

    #[test]
    fn renders_metric_names_with_custom_prefix() {
        let rendered = render_prometheus(&Metrics::default(), "nettest");

        assert!(rendered.contains("# TYPE nettest_bytes gauge\n"));
        assert!(rendered.contains("nettest_bandwidth 0\n"));
        assert!(!rendered.contains("iperf3_bytes"));
    }

    #[test]
    fn identifies_retryable_statuses() {
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(StatusCode::BAD_GATEWAY));
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn caps_retry_delay() {
        assert_eq!(retry_delay(0), Duration::from_millis(100));
        assert_eq!(retry_delay(1), Duration::from_millis(200));
        assert_eq!(retry_delay(10), Duration::from_secs(1));
    }
}
