use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use url::Url;

use crate::metrics::Metrics;

const PUSH_RETRY_BASE_DELAY: Duration = Duration::from_millis(100);
const PUSH_RETRY_MAX_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct PushGatewayConfig {
    pub endpoint: Url,
    pub job: String,
    pub labels: Vec<(String, String)>,
    pub timeout: Duration,
    pub retries: u32,
    pub user_agent: String,
    pub metric_prefix: String,
}

pub struct PushGateway {
    client: Client,
    url: Url,
    retries: u32,
    metric_prefix: String,
}

impl PushGateway {
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
            .context("failed to build HTTP client")?;

        Ok(Self {
            client,
            url,
            retries: config.retries,
            metric_prefix: config.metric_prefix,
        })
    }

    pub fn push(&self, metrics: &Metrics) -> Result<()> {
        let body = render_prometheus(metrics, &self.metric_prefix);
        for attempt in 0..=self.retries {
            match self.push_once(&body) {
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
                error: anyhow!("failed to send Pushgateway request: {err}"),
                retryable: true,
            })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(PushAttemptError {
                error: anyhow!("Pushgateway returned {status}"),
                retryable: is_retryable_status(status),
            });
        }

        Ok(())
    }
}

#[derive(Debug)]
struct PushAttemptError {
    error: anyhow::Error,
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
    gauge(
        &mut out,
        &metric_name(prefix, "tcp_retransmits"),
        metrics.tcp_retransmits,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "tcp_rtt_seconds"),
        metrics.tcp_rtt_seconds,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "tcp_rttvar_seconds"),
        metrics.tcp_rttvar_seconds,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "tcp_snd_cwnd_bytes"),
        metrics.tcp_snd_cwnd_bytes,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "tcp_snd_wnd_bytes"),
        metrics.tcp_snd_wnd_bytes,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "tcp_pmtu_bytes"),
        metrics.tcp_pmtu_bytes,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "tcp_reorder_events"),
        metrics.tcp_reorder_events,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "udp_packets"),
        metrics.udp_packets,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "udp_lost_packets"),
        metrics.udp_lost_packets,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "udp_jitter_seconds"),
        metrics.udp_jitter_seconds,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "udp_out_of_order_packets"),
        metrics.udp_out_of_order_packets,
    );
    gauge(&mut out, &metric_name(prefix, "omitted"), metrics.omitted);
    out
}

fn metric_name(prefix: &str, suffix: &str) -> String {
    format!("{prefix}_{suffix}")
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
                tcp_retransmits: 5.0,
                tcp_rtt_seconds: 0.006,
                tcp_rttvar_seconds: 0.007,
                tcp_snd_cwnd_bytes: 8.0,
                tcp_snd_wnd_bytes: 9.0,
                tcp_pmtu_bytes: 10.0,
                tcp_reorder_events: 11.0,
                udp_packets: 2.0,
                udp_lost_packets: 3.0,
                udp_jitter_seconds: 0.004,
                udp_out_of_order_packets: 12.0,
                omitted: 1.0,
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

        for name in [
            "iperf3_bytes",
            "iperf3_bandwidth",
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
            "iperf3_omitted",
        ] {
            assert!(rendered.contains(&format!("# TYPE {name} gauge\n")));
            assert!(rendered.contains(&format!("{name} 0\n")));
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
