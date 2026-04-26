//! Prometheus text exposition encoding for iperf metrics.

use crate::metrics::{Metrics, WindowGaugeStats, WindowMetrics};
use crate::{Error, Result};

/// Encoder for Prometheus text exposition snapshots.
///
/// The encoder is intentionally transport-agnostic: callers can write the
/// returned text to a file, serve it from their own HTTP endpoint, or pass it to
/// another delivery mechanism without enabling the `pushgateway` feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrometheusEncoder {
    metric_prefix: String,
    labels: Vec<(String, String)>,
}

impl PrometheusEncoder {
    /// Default metric prefix used by the CLI, [`PrometheusEncoder`], and
    /// Pushgateway helpers.
    pub const DEFAULT_PREFIX: &'static str = "iperf3";

    /// Build an encoder with a custom metric name prefix.
    pub fn new(metric_prefix: impl Into<String>) -> Result<Self> {
        let metric_prefix = metric_prefix.into();
        validate_metric_prefix(&metric_prefix)?;
        Ok(Self {
            metric_prefix,
            labels: Vec::new(),
        })
    }

    /// Build an encoder with fixed labels on every emitted sample.
    pub fn with_labels<I, K, V>(metric_prefix: impl Into<String>, labels: I) -> Result<Self>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let metric_prefix = metric_prefix.into();
        let labels = labels
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect::<Vec<_>>();
        validate_metric_prefix(&metric_prefix)?;
        validate_labels(&labels)?;
        Ok(Self {
            metric_prefix,
            labels,
        })
    }

    /// Return the metric prefix used by this encoder.
    pub fn metric_prefix(&self) -> &str {
        &self.metric_prefix
    }

    /// Return fixed labels applied to every emitted sample.
    pub fn labels(&self) -> &[(String, String)] {
        &self.labels
    }

    /// Encode one immediate interval sample as Prometheus text exposition.
    pub fn encode_interval(&self, metrics: &Metrics) -> String {
        render_interval_prometheus_with_labels(metrics, &self.metric_prefix, &self.labels)
    }

    /// Encode one aggregated window summary as Prometheus text exposition.
    pub fn encode_window(&self, metrics: &WindowMetrics) -> String {
        render_window_prometheus_with_labels(metrics, &self.metric_prefix, &self.labels)
    }
}

impl Default for PrometheusEncoder {
    fn default() -> Self {
        Self {
            metric_prefix: Self::DEFAULT_PREFIX.to_owned(),
            labels: Vec::new(),
        }
    }
}

pub(crate) fn validate_metric_prefix(prefix: &str) -> Result<()> {
    if !is_valid_metric_prefix(prefix) {
        return Err(Error::invalid_argument(format!(
            "invalid Prometheus metric prefix '{prefix}'"
        )));
    }
    Ok(())
}

fn is_valid_metric_prefix(prefix: &str) -> bool {
    is_valid_metric_prefix_bytes(prefix.as_bytes())
}

fn is_valid_metric_prefix_bytes(prefix: &[u8]) -> bool {
    let Some((&first, rest)) = prefix.split_first() else {
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

fn validate_labels(labels: &[(String, String)]) -> Result<()> {
    for (name, value) in labels {
        if !is_valid_label_name(name) {
            return Err(Error::invalid_argument(format!(
                "invalid Prometheus label name '{name}'"
            )));
        }
        if value.is_empty() {
            return Err(Error::invalid_argument(format!(
                "Prometheus label value for '{name}' must not be empty"
            )));
        }
    }
    for (index, (name, _)) in labels.iter().enumerate() {
        if labels[..index]
            .iter()
            .any(|(previous_name, _)| previous_name == name)
        {
            return Err(Error::invalid_argument(format!(
                "duplicate Prometheus label name '{name}'"
            )));
        }
    }
    Ok(())
}

fn is_valid_label_name(name: &str) -> bool {
    is_valid_label_name_bytes(name.as_bytes())
}

fn is_valid_label_name_bytes(name: &[u8]) -> bool {
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

#[cfg(feature = "pushgateway")]
pub(crate) fn render_interval_prometheus(metrics: &Metrics, prefix: &str) -> String {
    render_interval_prometheus_with_labels(metrics, prefix, &[])
}

fn render_interval_prometheus_with_labels(
    metrics: &Metrics,
    prefix: &str,
    labels: &[(String, String)],
) -> String {
    let mut out = String::new();
    let label_set = label_set(labels);
    gauge(
        &mut out,
        &metric_name(prefix, "transferred_bytes"),
        metrics.transferred_bytes,
        &label_set,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "bandwidth_bits_per_second"),
        metrics.bandwidth_bits_per_second,
        &label_set,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "stream_count"),
        metrics.stream_count as f64,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_retransmits"),
        metrics.tcp_retransmits,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_rtt_seconds"),
        metrics.tcp_rtt_seconds,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_rttvar_seconds"),
        metrics.tcp_rttvar_seconds,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_snd_cwnd_bytes"),
        metrics.tcp_snd_cwnd_bytes,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_snd_wnd_bytes"),
        metrics.tcp_snd_wnd_bytes,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_pmtu_bytes"),
        metrics.tcp_pmtu_bytes,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "tcp_reorder_events"),
        metrics.tcp_reorder_events,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "udp_packets"),
        metrics.udp_packets,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "udp_lost_packets"),
        metrics.udp_lost_packets,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "udp_jitter_seconds"),
        metrics.udp_jitter_seconds,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "udp_out_of_order_packets"),
        metrics.udp_out_of_order_packets,
        &label_set,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "omitted_intervals"),
        if metrics.omitted { 1.0 } else { 0.0 },
        &label_set,
    );
    out
}

#[cfg(feature = "pushgateway")]
pub(crate) fn render_window_prometheus(metrics: &WindowMetrics, prefix: &str) -> String {
    render_window_prometheus_with_labels(metrics, prefix, &[])
}

fn render_window_prometheus_with_labels(
    metrics: &WindowMetrics,
    prefix: &str,
    labels: &[(String, String)],
) -> String {
    let mut out = String::new();
    let label_set = label_set(labels);
    gauge(
        &mut out,
        &metric_name(prefix, "window_duration_seconds"),
        metrics.duration_seconds,
        &label_set,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "window_transferred_bytes"),
        metrics.transferred_bytes,
        &label_set,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_bandwidth",
        "bits_per_second",
        metrics.bandwidth_bits_per_second,
        &label_set,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_tcp_rtt",
        "seconds",
        metrics.tcp_rtt_seconds,
        &label_set,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_tcp_rttvar",
        "seconds",
        metrics.tcp_rttvar_seconds,
        &label_set,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_tcp_snd_cwnd",
        "bytes",
        metrics.tcp_snd_cwnd_bytes,
        &label_set,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_tcp_snd_wnd",
        "bytes",
        metrics.tcp_snd_wnd_bytes,
        &label_set,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_tcp_pmtu",
        "bytes",
        metrics.tcp_pmtu_bytes,
        &label_set,
    );
    gauge_stats(
        &mut out,
        prefix,
        "window_udp_jitter",
        "seconds",
        metrics.udp_jitter_seconds,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "window_tcp_retransmits"),
        metrics.tcp_retransmits,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "window_tcp_reorder_events"),
        metrics.tcp_reorder_events,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "window_udp_packets"),
        metrics.udp_packets,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "window_udp_lost_packets"),
        metrics.udp_lost_packets,
        &label_set,
    );
    gauge_option(
        &mut out,
        &metric_name(prefix, "window_udp_out_of_order_packets"),
        metrics.udp_out_of_order_packets,
        &label_set,
    );
    gauge(
        &mut out,
        &metric_name(prefix, "window_omitted_intervals"),
        metrics.omitted_intervals,
        &label_set,
    );
    out
}

fn metric_name(prefix: &str, suffix: &str) -> String {
    format!("{prefix}_{suffix}")
}

fn label_set(labels: &[(String, String)]) -> String {
    if labels.is_empty() {
        return String::new();
    }

    let mut out = String::from("{");
    for (index, (name, value)) in labels.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(name);
        out.push_str("=\"");
        push_escaped_label_value(&mut out, value);
        out.push('"');
    }
    out.push('}');
    out
}

fn push_escaped_label_value(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str(r#"\""#),
            '\n' => out.push_str(r"\n"),
            _ => out.push(ch),
        }
    }
}

fn gauge_stats(
    out: &mut String,
    prefix: &str,
    stem: &str,
    unit: &str,
    stats: WindowGaugeStats,
    label_set: &str,
) {
    if stats.samples == 0 {
        return;
    }
    gauge(
        out,
        &metric_name(prefix, &format!("{stem}_mean_{unit}")),
        stats.mean,
        label_set,
    );
    gauge(
        out,
        &metric_name(prefix, &format!("{stem}_min_{unit}")),
        stats.min,
        label_set,
    );
    gauge(
        out,
        &metric_name(prefix, &format!("{stem}_max_{unit}")),
        stats.max,
        label_set,
    );
}

fn gauge(out: &mut String, name: &str, value: f64, label_set: &str) {
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push_str(" gauge\n");
    out.push_str(name);
    out.push_str(label_set);
    out.push(' ');
    out.push_str(&value.to_string());
    out.push('\n');
}

fn gauge_option(out: &mut String, name: &str, value: Option<f64>, label_set: &str) {
    if let Some(value) = value {
        gauge(out, name, value, label_set);
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    #[kani::unwind(6)]
    fn metric_prefix_matches_documented_shape_for_bounded_ascii() {
        let len: usize = kani::any();
        kani::assume(len <= 5);
        let bytes: [u8; 5] = kani::any();
        let raw = &bytes[..len];

        let expected = if let Some((&first, rest)) = raw.split_first() {
            let mut ok = first.is_ascii_alphabetic() || first == b'_';
            for &byte in rest {
                ok &= byte.is_ascii_alphanumeric() || byte == b'_';
            }
            ok
        } else {
            false
        };

        assert_eq!(is_valid_metric_prefix_bytes(raw), expected);
    }

    #[kani::proof]
    #[kani::unwind(6)]
    fn label_name_matches_documented_shape_for_bounded_ascii() {
        let len: usize = kani::any();
        kani::assume(len <= 5);
        let bytes: [u8; 5] = kani::any();
        let raw = &bytes[..len];

        let expected = if let Some((&first, rest)) = raw.split_first() {
            let mut ok = first.is_ascii_alphabetic() || first == b'_';
            for &byte in rest {
                ok &= byte.is_ascii_alphanumeric() || byte == b'_';
            }
            ok
        } else {
            false
        };

        assert_eq!(is_valid_label_name_bytes(raw), expected);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_encoder_renders_prometheus_gauges() {
        let rendered = PrometheusEncoder::default().encode_interval(&Metrics {
            transferred_bytes: 1.0,
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
        });

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
    fn custom_prefix_changes_metric_names() {
        let rendered = PrometheusEncoder::new("nettest")
            .unwrap()
            .encode_interval(&Metrics::default());

        assert!(rendered.contains("# TYPE nettest_transferred_bytes gauge\n"));
        assert!(rendered.contains("nettest_bandwidth_bits_per_second 0\n"));
        assert!(!rendered.contains("iperf3_transferred_bytes"));
    }

    #[test]
    fn fixed_labels_are_rendered_on_samples() {
        let rendered = PrometheusEncoder::with_labels(
            "nettest",
            [("site", "ci"), ("case", "quote\"slash\\line\n")],
        )
        .unwrap()
        .encode_interval(&Metrics::default());

        assert!(rendered.contains("# TYPE nettest_transferred_bytes gauge\n"));
        assert!(rendered.contains(
            "nettest_transferred_bytes{site=\"ci\",case=\"quote\\\"slash\\\\line\\n\"} 0\n"
        ));
    }

    #[test]
    fn invalid_prefix_is_rejected() {
        let err = PrometheusEncoder::new("bad-prefix").unwrap_err();

        assert!(err.to_string().contains("metric prefix"));
    }

    #[test]
    fn invalid_labels_are_rejected() {
        for labels in [
            vec![("9bad", "value")],
            vec![("ok", "")],
            vec![("dup", "one"), ("dup", "two")],
        ] {
            let err = PrometheusEncoder::with_labels("iperf3", labels).unwrap_err();
            assert!(err.to_string().contains("label"));
        }
    }

    #[test]
    fn renders_all_expected_metric_names() {
        let rendered = PrometheusEncoder::default().encode_interval(&Metrics::default());

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
        let rendered = PrometheusEncoder::default().encode_window(&WindowMetrics {
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
        });

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
        let rendered = PrometheusEncoder::default().encode_window(&WindowMetrics::default());

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
}
