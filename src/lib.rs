#![warn(missing_docs)]

//! Rust frontend APIs for driving upstream libiperf.
//!
//! `iperf3-rs` links upstream `esnet/iperf3` through FFI. The high-level
//! [`IperfCommand`] API accepts ordinary iperf arguments, lets libiperf parse
//! and run them, and can stream the same live interval metrics used by the CLI's
//! Pushgateway exporter.
//!
//! This crate is useful when a Rust program needs to run iperf tests directly,
//! for example from a bot, controller, or test harness, without spawning an
//! external `iperf3-rs` process and parsing stdout.
//!
//! # Examples
//!
//! Run a client and consume live interval metrics:
//!
//! ```no_run
//! use iperf3_rs::{IperfCommand, MetricEvent, MetricsMode, Result};
//!
//! fn main() -> Result<()> {
//!     let mut command = IperfCommand::new();
//!     command
//!         .args(["-c", "127.0.0.1", "-t", "10", "-i", "1"])
//!         .metrics(MetricsMode::Interval);
//!
//!     let mut running = command.spawn()?;
//!     let mut metrics = running.take_metrics().expect("metrics enabled");
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

mod args;
#[doc(hidden)]
pub mod cli;
mod command;
mod error;
mod help;

mod iperf;
mod metrics;
mod pushgateway;
mod version;

pub use command::{IperfCommand, IperfResult, RunningIperf};
pub use error::{Error, ErrorKind, Result};
pub use iperf::{Role, libiperf_version, usage_long};
pub use metrics::{
    MetricEvent, Metrics, MetricsMode, MetricsStream, WindowGaugeStats, WindowMetrics,
    aggregate_window,
};
pub use pushgateway::{PushGateway, PushGatewayConfig};
