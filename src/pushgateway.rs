use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::blocking::Client;
use url::Url;

use crate::metrics::Metrics;

const PUSH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct PushGatewayConfig {
    pub endpoint: Url,
    pub job: String,
    pub labels: Vec<(String, String)>,
}

pub struct PushGateway {
    client: Client,
    url: Url,
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
            .timeout(PUSH_TIMEOUT)
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self { client, url })
    }

    pub fn push(&self, metrics: &Metrics) -> Result<()> {
        let body = render_prometheus(metrics);
        let response = self
            .client
            .put(self.url.clone())
            .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
            .body(body)
            .send()
            .context("failed to send Pushgateway request")?;

        if !response.status().is_success() {
            return Err(anyhow!("Pushgateway returned {}", response.status()));
        }

        Ok(())
    }
}

fn render_prometheus(metrics: &Metrics) -> String {
    let mut out = String::new();
    gauge(&mut out, "iperf3_bytes", metrics.bytes);
    gauge(
        &mut out,
        "iperf3_bandwidth",
        metrics.bandwidth_bits_per_second,
    );
    gauge(&mut out, "iperf3_packets", metrics.packets);
    gauge(&mut out, "iperf3_error_packets", metrics.error_packets);
    gauge(&mut out, "iperf3_jitter", metrics.jitter_seconds);
    gauge(&mut out, "iperf3_tcp_retransmits", metrics.tcp_retransmits);
    out
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
        let rendered = render_prometheus(&Metrics {
            bytes: 1.0,
            bandwidth_bits_per_second: 8.0,
            packets: 2.0,
            error_packets: 3.0,
            jitter_seconds: 0.004,
            tcp_retransmits: 5.0,
        });
        assert!(rendered.contains("iperf3_bytes 1\n"));
        assert!(rendered.contains("iperf3_jitter 0.004\n"));
    }

    #[test]
    fn builds_pushgateway_grouping_url() {
        let gateway = PushGateway::new(PushGatewayConfig {
            endpoint: Url::parse("http://127.0.0.1:9091/base/").unwrap(),
            job: "iperf job".to_owned(),
            labels: vec![
                ("test".to_owned(), "test/one".to_owned()),
                ("scenario".to_owned(), "sample#1".to_owned()),
                ("iperf_mode".to_owned(), "client".to_owned()),
            ],
        })
        .unwrap();

        assert_eq!(
            gateway.url.as_str(),
            "http://127.0.0.1:9091/base/metrics/job/iperf%20job/test/test%2Fone/scenario/sample%231/iperf_mode/client"
        );
    }

    #[test]
    fn renders_all_expected_metric_names() {
        let rendered = render_prometheus(&Metrics::default());

        for name in [
            "iperf3_bytes",
            "iperf3_bandwidth",
            "iperf3_packets",
            "iperf3_error_packets",
            "iperf3_jitter",
            "iperf3_tcp_retransmits",
        ] {
            assert!(rendered.contains(&format!("# TYPE {name} gauge\n")));
            assert!(rendered.contains(&format!("{name} 0\n")));
        }
    }
}
