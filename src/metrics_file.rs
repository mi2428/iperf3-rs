//! File-backed metrics output.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::metrics::{MetricEvent, Metrics, WindowMetrics};
use crate::prometheus::PrometheusEncoder;
use crate::{Error, ErrorKind, Result};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// File output format for metrics snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetricsFileFormat {
    /// Append one JSON object per interval.
    Jsonl,
    /// Replace the file with the latest Prometheus text exposition snapshot.
    Prometheus,
}

impl MetricsFileFormat {
    /// Parse a CLI-compatible metrics file format name.
    pub fn parse(raw: &str) -> Option<Self> {
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
///
/// JSONL output appends one object per event. Prometheus output atomically
/// replaces the file with the latest encoded snapshot on each write.
#[derive(Debug, Clone)]
pub struct MetricsFileSink {
    path: PathBuf,
    format: MetricsFileFormat,
    encoder: PrometheusEncoder,
}

impl MetricsFileSink {
    /// Create a sink with the default Prometheus metric prefix.
    pub fn new(path: impl Into<PathBuf>, format: MetricsFileFormat) -> Result<Self> {
        Self::with_prefix(path, format, PrometheusEncoder::DEFAULT_PREFIX)
    }

    /// Create a sink with a custom Prometheus metric prefix.
    pub fn with_prefix(
        path: impl Into<PathBuf>,
        format: MetricsFileFormat,
        metric_prefix: impl Into<String>,
    ) -> Result<Self> {
        let sink = Self {
            path: path.into(),
            format,
            encoder: PrometheusEncoder::new(metric_prefix)?,
        };
        sink.create_empty_file()?;
        Ok(sink)
    }

    /// Return the output path this sink writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the configured output format.
    pub fn format(&self) -> MetricsFileFormat {
        self.format
    }

    /// Write one metrics stream event.
    pub fn write_event(&self, event: &MetricEvent) -> Result<()> {
        match event {
            MetricEvent::Interval(metrics) => self.write_interval(metrics),
            MetricEvent::Window(metrics) => self.write_window(metrics),
        }
    }

    /// Write one immediate interval sample.
    pub fn write_interval(&self, metrics: &Metrics) -> Result<()> {
        match self.format {
            MetricsFileFormat::Jsonl => self.append_jsonl("interval", metrics),
            MetricsFileFormat::Prometheus => self.write_prometheus(metrics),
        }
    }

    /// Write one aggregated window summary.
    pub fn write_window(&self, metrics: &WindowMetrics) -> Result<()> {
        match self.format {
            MetricsFileFormat::Jsonl => self.append_jsonl("window", metrics),
            MetricsFileFormat::Prometheus => self.write_window_prometheus(metrics),
        }
    }

    fn create_empty_file(&self) -> Result<()> {
        File::create(&self.path)
            .map(|_| ())
            .map_err(|err| file_error("failed to create metrics file", &self.path, err))
    }

    fn append_jsonl<T>(&self, event: &'static str, metrics: &T) -> Result<()>
    where
        T: Serialize,
    {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|err| file_error("failed to open metrics file", &self.path, err))?;
        serde_json::to_writer(&mut file, &JsonlEvent { event, metrics }).map_err(|err| {
            Error::with_source(ErrorKind::MetricsFile, "failed to encode metrics JSON", err)
        })?;
        file.write_all(b"\n")
            .map_err(|err| file_error("failed to write metrics file", &self.path, err))
    }

    fn write_prometheus(&self, metrics: &Metrics) -> Result<()> {
        atomic_write(&self.path, self.encoder.encode_interval(metrics).as_bytes())
    }

    fn write_window_prometheus(&self, metrics: &WindowMetrics) -> Result<()> {
        atomic_write(&self.path, self.encoder.encode_window(metrics).as_bytes())
    }
}

#[derive(Serialize)]
struct JsonlEvent<'a, T> {
    event: &'static str,
    #[serde(flatten)]
    metrics: &'a T,
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

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let temp_path = temp_path_for(path);
    let result = write_temp_then_rename(&temp_path, path, contents);
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result.map_err(|err| file_error("failed to write metrics file", path, err))
}

fn write_temp_then_rename(temp_path: &Path, path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)?;
    file.write_all(contents)?;
    file.flush()?;
    drop(file);
    fs::rename(temp_path, path)
}

fn temp_path_for(path: &Path) -> PathBuf {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "metrics".into());
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{file_name}.tmp-{}-{counter}", std::process::id()))
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
        let sink = MetricsFileSink::new(&path, MetricsFileFormat::Jsonl).unwrap();

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
        let sink =
            MetricsFileSink::with_prefix(&path, MetricsFileFormat::Prometheus, "nettest").unwrap();

        sink.write_interval(&sample_metrics(1.0)).unwrap();
        sink.write_interval(&sample_metrics(2.0)).unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("nettest_bytes 2\n"));
        assert!(!contents.contains("nettest_bytes 1\n"));
        assert_no_temp_files_for(&path);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn write_event_supports_window_jsonl() {
        let path = temp_path("jsonl");
        let sink = MetricsFileSink::new(&path, MetricsFileFormat::Jsonl).unwrap();

        sink.write_event(&MetricEvent::Window(WindowMetrics {
            duration_seconds: 2.0,
            transferred_bytes: 64.0,
            ..WindowMetrics::default()
        }))
        .unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains(r#""event":"window""#));
        assert!(contents.contains(r#""duration_seconds":2.0"#));
        assert!(contents.contains(r#""transferred_bytes":64.0"#));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn write_interval_reports_file_errors() {
        let path = temp_path("jsonl");
        let sink = MetricsFileSink::new(&path, MetricsFileFormat::Jsonl).unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();

        let err = sink.write_interval(&sample_metrics(1.0)).unwrap_err();

        assert_eq!(err.kind(), ErrorKind::MetricsFile);
        assert!(
            err.to_string().contains("failed to open metrics file"),
            "{err:#}"
        );
        let _ = fs::remove_dir(path);
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

    fn assert_no_temp_files_for(path: &Path) {
        let parent = path.parent().unwrap();
        let file_name = path.file_name().unwrap().to_string_lossy();
        let prefix = format!(".{file_name}.tmp-");
        let leftovers = fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .collect::<Vec<_>>();
        assert!(
            leftovers.is_empty(),
            "Prometheus atomic writes should not leave temp files"
        );
    }
}
