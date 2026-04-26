//! Pushgateway HTTP delivery helpers.
//!
//! The CLI uses these types internally, and library users can also construct a
//! [`PushGateway`] when they want to push [`crate::Metrics`] or
//! [`crate::WindowMetrics`] collected from [`crate::MetricsStream`].

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::Client;
use url::Url;

use crate::metrics::{Metrics, WindowMetrics};
use crate::prometheus::{
    PrometheusEncoder, render_interval_prometheus as render_prometheus, render_window_prometheus,
    validate_metric_prefix,
};
use crate::{Error, ErrorKind, Result};

const PUSH_RETRY_BASE_DELAY: Duration = Duration::from_millis(100);
const PUSH_RETRY_MAX_DELAY: Duration = Duration::from_secs(1);

/// Configuration for a [`PushGateway`] sink.
#[derive(Debug, Clone)]
#[non_exhaustive]
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
    /// Delete this grouping key from Pushgateway after the run finishes.
    pub delete_on_finish: bool,
}

impl PushGatewayConfig {
    /// Default Pushgateway job name used by the CLI and builder.
    pub const DEFAULT_JOB: &'static str = "iperf3";
    /// Default metric prefix used by the CLI and builder.
    pub const DEFAULT_METRIC_PREFIX: &'static str = PrometheusEncoder::DEFAULT_PREFIX;
    /// Default number of Pushgateway retries after the first failed request.
    pub const DEFAULT_RETRIES: u32 = 0;
    /// Maximum supported retry count.
    pub const MAX_RETRIES: u32 = 10;

    /// Build a config with production-safe defaults for every field except the
    /// Pushgateway endpoint.
    pub fn new(endpoint: Url) -> Self {
        Self {
            endpoint,
            job: Self::DEFAULT_JOB.to_owned(),
            labels: Vec::new(),
            timeout: Self::default_timeout(),
            retries: Self::DEFAULT_RETRIES,
            user_agent: Self::default_user_agent(),
            metric_prefix: Self::DEFAULT_METRIC_PREFIX.to_owned(),
            delete_on_finish: false,
        }
    }

    /// Parse a Pushgateway endpoint, defaulting bare `host:port` values to HTTP.
    pub fn parse_endpoint(raw: &str) -> Result<Url> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(Error::invalid_argument(
                "Pushgateway endpoint must not be empty",
            ));
        }

        // Keep local development terse: `localhost:9091` means the normal HTTP
        // Pushgateway endpoint unless a scheme is explicitly provided.
        let with_scheme = if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.to_owned()
        } else {
            format!("http://{raw}")
        };
        Url::parse(&with_scheme).map_err(|err| {
            Error::with_source(
                ErrorKind::InvalidArgument,
                "invalid Pushgateway endpoint URL",
                err,
            )
        })
    }

    /// Default per-request timeout.
    pub const fn default_timeout() -> Duration {
        Duration::from_secs(5)
    }

    /// Default HTTP `User-Agent`.
    pub fn default_user_agent() -> String {
        format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    }

    /// Set the Pushgateway job name.
    pub fn job(mut self, job: impl Into<String>) -> Self {
        self.job = job.into();
        self
    }

    /// Add one grouping label.
    pub fn label(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.push((name.into(), value.into()));
        self
    }

    /// Replace grouping labels.
    pub fn labels<I, K, V>(mut self, labels: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.labels = labels
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        self
    }

    /// Set the per-request HTTP timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the number of retries after the first failed request.
    pub fn retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// Set the HTTP `User-Agent`.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    /// Set the Prometheus metric name prefix.
    pub fn metric_prefix(mut self, metric_prefix: impl Into<String>) -> Self {
        self.metric_prefix = metric_prefix.into();
        self
    }

    /// Delete this grouping key from Pushgateway after direct delivery finishes.
    pub fn delete_on_finish(mut self, delete: bool) -> Self {
        self.delete_on_finish = delete;
        self
    }

    /// Validate this config before it is used for HTTP delivery.
    pub fn validate(&self) -> Result<()> {
        validate_endpoint(&self.endpoint)?;
        validate_job(&self.job)?;
        validate_labels(&self.labels)?;
        validate_timeout(self.timeout)?;
        validate_retries(self.retries)?;
        validate_user_agent(&self.user_agent)?;
        validate_metric_prefix(&self.metric_prefix)?;
        Ok(())
    }
}

/// HTTP sink for pushing iperf metrics to Prometheus Pushgateway.
pub struct PushGateway {
    client: Client,
    url: Url,
    retries: u32,
    metric_prefix: String,
    delete_on_finish: bool,
}

impl PushGateway {
    /// Build a Pushgateway sink from validated configuration.
    pub fn new(config: PushGatewayConfig) -> Result<Self> {
        config.validate()?;

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
            delete_on_finish: config.delete_on_finish,
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

    /// Delete this sink's grouping key from Pushgateway.
    pub fn delete(&self) -> Result<()> {
        for attempt in 0..=self.retries {
            match self.delete_once() {
                Ok(()) => return Ok(()),
                Err(err) if err.retryable && attempt < self.retries => {
                    std::thread::sleep(retry_delay(attempt));
                }
                Err(err) => return Err(err.error),
            }
        }

        unreachable!("delete retry loop always returns")
    }

    pub(crate) fn delete_on_finish(&self) -> bool {
        self.delete_on_finish
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

    fn delete_once(&self) -> std::result::Result<(), PushAttemptError> {
        let response =
            self.client
                .delete(self.url.clone())
                .send()
                .map_err(|err| PushAttemptError {
                    error: Error::with_source(
                        ErrorKind::PushGateway,
                        "failed to send Pushgateway delete request",
                        err,
                    ),
                    retryable: true,
                })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(PushAttemptError {
                error: Error::pushgateway(format!("Pushgateway delete returned {status}")),
                retryable: is_retryable_status(status),
            });
        }

        Ok(())
    }
}

fn validate_endpoint(endpoint: &Url) -> Result<()> {
    match endpoint.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(Error::invalid_argument(format!(
                "Pushgateway endpoint scheme must be http or https, got '{scheme}'"
            )));
        }
    }
    if endpoint.host_str().is_none() {
        return Err(Error::invalid_argument(
            "Pushgateway endpoint must include a host",
        ));
    }
    Ok(())
}

fn validate_job(job: &str) -> Result<()> {
    if job.is_empty() {
        return Err(Error::invalid_argument(
            "Pushgateway job name must not be empty",
        ));
    }
    Ok(())
}

fn validate_labels(labels: &[(String, String)]) -> Result<()> {
    for (name, value) in labels {
        validate_label(name, value)?;
    }
    reject_duplicate_labels(labels)?;
    Ok(())
}

pub(crate) fn validate_label(name: &str, value: &str) -> Result<()> {
    if !is_valid_label_name(name) {
        return Err(Error::invalid_argument(format!(
            "invalid Pushgateway label name '{name}'"
        )));
    }
    if is_reserved_label_name(name) {
        return Err(Error::invalid_argument(format!(
            "Pushgateway label name '{name}' is reserved"
        )));
    }
    if value.is_empty() {
        return Err(Error::invalid_argument(format!(
            "Pushgateway label value for '{name}' must not be empty"
        )));
    }
    Ok(())
}

fn reject_duplicate_labels(labels: &[(String, String)]) -> Result<()> {
    for (index, (name, _)) in labels.iter().enumerate() {
        if labels[..index]
            .iter()
            .any(|(previous_name, _)| previous_name == name)
        {
            return Err(Error::invalid_argument(format!(
                "duplicate Pushgateway label name '{name}'"
            )));
        }
    }
    Ok(())
}

fn validate_timeout(timeout: Duration) -> Result<()> {
    if timeout.is_zero() {
        return Err(Error::invalid_argument(
            "Pushgateway timeout must be greater than zero",
        ));
    }
    Ok(())
}

pub(crate) fn validate_retries(retries: u32) -> Result<()> {
    if retries > PushGatewayConfig::MAX_RETRIES {
        return Err(Error::invalid_argument(format!(
            "Pushgateway retries must be at most {}",
            PushGatewayConfig::MAX_RETRIES
        )));
    }
    Ok(())
}

pub(crate) fn validate_user_agent(value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::invalid_argument(
            "Pushgateway User-Agent must not be empty",
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(Error::invalid_argument(
            "Pushgateway User-Agent must not contain control characters",
        ));
    }
    Ok(())
}

pub(crate) fn is_valid_label_name(name: &str) -> bool {
    is_valid_label_name_bytes(name.as_bytes())
}

pub(crate) fn is_reserved_label_name(name: &str) -> bool {
    is_reserved_label_name_bytes(name.as_bytes())
}

pub(crate) fn is_reserved_label_name_bytes(name: &[u8]) -> bool {
    name == b"job"
}

pub(crate) fn is_valid_label_name_bytes(name: &[u8]) -> bool {
    let Some((&first, rest)) = name.split_first() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    for &byte in rest {
        if !(byte.is_ascii_alphanumeric() || byte == b'_') {
            return false;
        }
    }
    true
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use crate::metrics::WindowGaugeStats;

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
                omitted: true,
                ..Metrics::default()
            },
            "iperf3",
        );
        assert!(rendered.contains("iperf3_transferred_bytes 1\n"));
        assert!(rendered.contains("iperf3_stream_count 0\n"));
        assert!(rendered.contains("iperf3_tcp_rtt_seconds 0.006\n"));
        assert!(rendered.contains("iperf3_udp_packets 2\n"));
        assert!(rendered.contains("iperf3_udp_lost_packets 3\n"));
        assert!(rendered.contains("iperf3_udp_jitter_seconds 0.004\n"));
        assert!(rendered.contains("iperf3_udp_out_of_order_packets 12\n"));
        assert!(rendered.contains("iperf3_omitted_intervals 1\n"));
    }

    #[test]
    fn builds_pushgateway_grouping_url() {
        let config = PushGatewayConfig::new(Url::parse("http://127.0.0.1:9091/base/").unwrap())
            .job("iperf job")
            .label("test", "test/one")
            .label("scenario", "sample#1")
            .label("mode", "client")
            .user_agent("iperf3-rs/test");
        let gateway = PushGateway::new(config).unwrap();

        assert_eq!(
            gateway.url.as_str(),
            "http://127.0.0.1:9091/base/metrics/job/iperf%20job/test/test%2Fone/scenario/sample%231/mode/client"
        );
    }

    #[test]
    fn config_builder_sets_defaults() {
        let config = PushGatewayConfig::new(Url::parse("http://localhost:9091").unwrap());

        assert_eq!(config.job, PushGatewayConfig::DEFAULT_JOB);
        assert!(config.labels.is_empty());
        assert_eq!(config.timeout, PushGatewayConfig::default_timeout());
        assert_eq!(config.retries, PushGatewayConfig::DEFAULT_RETRIES);
        assert_eq!(config.user_agent, PushGatewayConfig::default_user_agent());
        assert_eq!(
            config.metric_prefix,
            PushGatewayConfig::DEFAULT_METRIC_PREFIX
        );
        assert!(!config.delete_on_finish);
        config.validate().unwrap();

        let delete_config = config.delete_on_finish(true);
        assert!(delete_config.delete_on_finish);
    }

    #[test]
    fn config_validation_rejects_values_cli_would_reject() {
        let endpoint = Url::parse("http://localhost:9091").unwrap();

        for (label, config, expected) in [
            (
                "empty job",
                PushGatewayConfig::new(endpoint.clone()).job(""),
                "job name",
            ),
            (
                "bad label",
                PushGatewayConfig::new(endpoint.clone()).label("9bad", "value"),
                "label name",
            ),
            (
                "reserved label",
                PushGatewayConfig::new(endpoint.clone()).label("job", "value"),
                "reserved",
            ),
            (
                "duplicate label",
                PushGatewayConfig::new(endpoint.clone())
                    .label("site", "a")
                    .label("site", "b"),
                "duplicate",
            ),
            (
                "zero timeout",
                PushGatewayConfig::new(endpoint.clone()).timeout(Duration::ZERO),
                "timeout",
            ),
            (
                "too many retries",
                PushGatewayConfig::new(endpoint.clone())
                    .retries(PushGatewayConfig::MAX_RETRIES + 1),
                "retries",
            ),
            (
                "bad prefix",
                PushGatewayConfig::new(endpoint.clone()).metric_prefix("bad-prefix"),
                "metric prefix",
            ),
        ] {
            let err = config.validate().expect_err(label);
            assert!(
                err.to_string().contains(expected),
                "{label} should mention {expected:?}, got {err}"
            );
        }
    }

    #[test]
    fn parses_bare_pushgateway_endpoint_as_http() {
        let url = PushGatewayConfig::parse_endpoint("localhost:9091").unwrap();

        assert_eq!(url.as_str(), "http://localhost:9091/");
    }

    #[test]
    fn delete_sends_http_delete_to_grouping_url() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 1024];
            let n = stream.read(&mut buffer).unwrap();
            stream
                .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            String::from_utf8_lossy(&buffer[..n]).into_owned()
        });

        let gateway =
            PushGateway::new(PushGatewayConfig::new(endpoint).label("scenario", "delete")).unwrap();
        gateway.delete().unwrap();

        let request = handle.join().unwrap();
        assert!(request.starts_with("DELETE /metrics/job/iperf3/scenario/delete HTTP/1.1"));
    }

    #[test]
    fn renders_all_expected_metric_names() {
        let rendered = render_prometheus(&Metrics::default(), "iperf3");

        for name in [
            "iperf3_transferred_bytes",
            "iperf3_bandwidth_bits_per_second",
            "iperf3_stream_count",
            "iperf3_omitted_intervals",
        ] {
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
                bandwidth_bits_per_second: WindowGaugeStats {
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
        assert!(rendered.contains("iperf3_window_bandwidth_mean_bits_per_second 100\n"));
        assert!(rendered.contains("iperf3_window_bandwidth_min_bits_per_second 90\n"));
        assert!(rendered.contains("iperf3_window_bandwidth_max_bits_per_second 110\n"));
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
            "iperf3_window_bandwidth_mean_bits_per_second",
            "iperf3_window_bandwidth_min_bits_per_second",
            "iperf3_window_bandwidth_max_bits_per_second",
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

        assert!(rendered.contains("# TYPE nettest_transferred_bytes gauge\n"));
        assert!(rendered.contains("nettest_bandwidth_bits_per_second 0\n"));
        assert!(!rendered.contains("iperf3_transferred_bytes"));
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
