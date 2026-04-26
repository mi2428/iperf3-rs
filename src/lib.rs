#![warn(missing_docs)]

//! Rust frontend APIs for driving upstream libiperf.
//!
//! `iperf3-rs` links upstream `esnet/iperf3` through FFI. The high-level
//! [`IperfCommand`] API accepts ordinary iperf arguments, lets libiperf parse
//! and run them, and can stream the same live interval metrics used by the CLI's
//! Pushgateway exporter.
//! Protocol-specific metrics are optional, so callers can distinguish a real
//! zero from values that libiperf did not report for a TCP, UDP, or SCTP run.
//!
//! This crate is useful when a Rust program needs to run iperf tests directly,
//! for example from a bot, controller, or test harness, without spawning an
//! external `iperf3-rs` process and parsing stdout.
//! High-level library runs suppress libiperf's ordinary stdout output by
//! default; use [`IperfCommand::inherit_output`] or [`IperfCommand::logfile`]
//! when an application intentionally wants upstream text output.
//!
//! # Examples
//!
//! Run a client and consume live interval metrics:
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use iperf3_rs::{IperfCommand, MetricEvent, MetricsMode, Result};
//!
//! fn main() -> Result<()> {
//!     let mut command = IperfCommand::client("127.0.0.1");
//!     command
//!         .duration(Duration::from_secs(10))
//!         .report_interval(Duration::from_secs(1));
//!
//!     let (running, mut metrics) = command.spawn_with_metrics(MetricsMode::Interval)?;
//!
//!     while let Some(event) = metrics.recv() {
//!         match event {
//!             MetricEvent::Interval(sample) => {
//!                 println!("{} bit/s", sample.bandwidth_bits_per_second);
//!             }
//!             MetricEvent::Window(window) => {
//!                 println!("{} bytes", window.transferred_bytes);
//!             }
//!             _ => {}
//!         }
//!     }
//!
//!     running.wait()?;
//!     Ok(())
//! }
//! ```
//!
//! # Concurrency
//!
//! High-level [`IperfCommand`] runs are serialized inside the process. libiperf
//! has process-global state for errors, signal handling, and output hooks, so
//! this crate avoids promising in-process parallelism that upstream does not
//! clearly guarantee. Server runs must use iperf's one-off mode (`-s -1`) by
//! default; opt in with [`IperfCommand::allow_unbounded_server`] only when the
//! process is dedicated to that long-lived server. Use separate processes for
//! parallel independent tests.

#[cfg(all(feature = "pushgateway", feature = "serde"))]
mod args;
#[cfg(all(feature = "pushgateway", feature = "serde"))]
mod cli;
mod command;
mod error;
#[cfg(all(feature = "pushgateway", feature = "serde"))]
mod help;

mod iperf;
mod metrics;
#[cfg(feature = "serde")]
mod metrics_file;
mod prometheus;
#[cfg(feature = "pushgateway")]
mod pushgateway;
#[cfg(all(feature = "pushgateway", feature = "serde"))]
mod version;

pub use command::{IperfCommand, IperfResult, RunningIperf};
pub use error::{Error, ErrorKind, Result};
pub use iperf::{Role, libiperf_version, usage_long};
pub use metrics::{
    MetricDirection, MetricEvent, Metrics, MetricsMode, MetricsStream, TransportProtocol,
    WindowGaugeStats, WindowMetrics, aggregate_window,
};
#[cfg(feature = "serde")]
pub use metrics_file::{MetricsFileFormat, MetricsFileSink};
pub use prometheus::PrometheusEncoder;
#[cfg(feature = "pushgateway")]
pub use pushgateway::{PushGateway, PushGatewayConfig};

#[cfg(all(feature = "pushgateway", feature = "serde"))]
#[doc(hidden)]
pub fn __private_cli_main() -> std::process::ExitCode {
    cli::main()
}
