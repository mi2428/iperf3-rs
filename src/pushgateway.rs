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
    pub test: String,
    pub scenario: String,
    pub mode: String,
}

pub struct PushGateway {
    client: Client,
    url: Url,
}

impl PushGateway {
    pub fn new(config: PushGatewayConfig) -> Result<Self> {
        let mut url = config.endpoint;
        let mut path = url.path().trim_end_matches('/').to_owned();
        path.push_str("/metrics/job/");
        path.push_str(&encode_path_segment(&config.job));
        path.push_str("/test/");
        path.push_str(&encode_path_segment(&config.test));
        path.push_str("/scenario/");
        path.push_str(&encode_path_segment(&config.scenario));
        path.push_str("/iperf_mode/");
        path.push_str(&encode_path_segment(&config.mode));
        url.set_path(&path);

        let client = Client::builder()
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
    let mut encoded = String::new();
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

