//! File-backed CLI metrics output.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::metrics::Metrics;
use crate::pushgateway::render_prometheus;
use crate::{Error, ErrorKind, Result};

/// File output format for CLI metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetricsFileFormat {
    /// Append one JSON object per interval.
    Jsonl,
    /// Replace the file with the latest Prometheus text exposition snapshot.
    Prometheus,
}

impl MetricsFileFormat {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        Self::parse_trimmed_bytes(raw.trim().as_bytes())
    }

    fn parse_trimmed_bytes(raw: &[u8]) -> Option<Self> {
        match raw {
            b"jsonl" => Some(Self::Jsonl),
            b"prometheus" => Some(Self::Prometheus),
            _ => None,
        }
    }
}

/// Writer for one metrics output file.
pub(crate) struct MetricsFileSink {
    path: PathBuf,
    format: MetricsFileFormat,
    metric_prefix: String,
}

impl MetricsFileSink {
    pub(crate) fn new(
        path: impl Into<PathBuf>,
        format: MetricsFileFormat,
        metric_prefix: impl Into<String>,
    ) -> Result<Self> {
        let sink = Self {
            path: path.into(),
            format,
            metric_prefix: metric_prefix.into(),
        };
        sink.create_empty_file()?;
        Ok(sink)
    }

    pub(crate) fn write_interval(&self, metrics: &Metrics) -> Result<()> {
        match self.format {
            MetricsFileFormat::Jsonl => self.append_jsonl(metrics),
            MetricsFileFormat::Prometheus => self.write_prometheus(metrics),
        }
    }

    fn create_empty_file(&self) -> Result<()> {
        File::create(&self.path)
            .map(|_| ())
            .map_err(|err| file_error("failed to create metrics file", &self.path, err))
    }

    fn append_jsonl(&self, metrics: &Metrics) -> Result<()> {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|err| file_error("failed to open metrics file", &self.path, err))?;
        serde_json::to_writer(
            &mut file,
            &JsonlInterval {
                event: "interval",
                metrics,
            },
        )
        .map_err(|err| {
            Error::with_source(ErrorKind::MetricsFile, "failed to encode metrics JSON", err)
        })?;
        file.write_all(b"\n")
            .map_err(|err| file_error("failed to write metrics file", &self.path, err))
    }

    fn write_prometheus(&self, metrics: &Metrics) -> Result<()> {
        fs::write(&self.path, render_prometheus(metrics, &self.metric_prefix))
            .map_err(|err| file_error("failed to write metrics file", &self.path, err))
    }
}

#[derive(Serialize)]
struct JsonlInterval<'a> {
    event: &'static str,
    #[serde(flatten)]
    metrics: &'a Metrics,
}

fn file_error(
    message: &'static str,
    path: &Path,
    source: impl std::error::Error + Send + Sync + 'static,
) -> Error {
    Error::with_source(
        ErrorKind::MetricsFile,
        format!("{message}: {}", path.display()),
        source,
    )
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    #[kani::unwind(12)]
    fn metrics_format_parser_matches_documented_values_for_bounded_bytes() {
        let len: usize = kani::any();
        kani::assume(len <= 10);
        let bytes: [u8; 10] = kani::any();
        let raw = &bytes[..len];

        let expected = match raw {
            b"jsonl" => Some(MetricsFileFormat::Jsonl),
            b"prometheus" => Some(MetricsFileFormat::Prometheus),
            _ => None,
        };

        assert_eq!(MetricsFileFormat::parse_trimmed_bytes(raw), expected);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn jsonl_format_appends_interval_events() {
        let path = temp_path("jsonl");
        let sink = MetricsFileSink::new(&path, MetricsFileFormat::Jsonl, "iperf3").unwrap();

        sink.write_interval(&sample_metrics(1.0)).unwrap();
        sink.write_interval(&sample_metrics(2.0)).unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(r#""event":"interval""#));
        assert!(lines[0].contains(r#""bytes":1.0"#));
        assert!(lines[1].contains(r#""bytes":2.0"#));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn prometheus_format_replaces_latest_snapshot() {
        let path = temp_path("prom");
        let sink = MetricsFileSink::new(&path, MetricsFileFormat::Prometheus, "nettest").unwrap();

        sink.write_interval(&sample_metrics(1.0)).unwrap();
        sink.write_interval(&sample_metrics(2.0)).unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("nettest_bytes 2\n"));
        assert!(!contents.contains("nettest_bytes 1\n"));
        let _ = fs::remove_file(path);
    }

    fn sample_metrics(bytes: f64) -> Metrics {
        Metrics {
            bytes,
            bandwidth_bits_per_second: bytes * 8.0,
            interval_duration_seconds: 1.0,
            ..Metrics::default()
        }
    }

    fn temp_path(extension: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "iperf3-rs-metrics-file-{}-{nonce}.{extension}",
            std::process::id()
        ))
    }
}
