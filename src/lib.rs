//! Rust bindings and helpers for driving upstream libiperf from Rust.
//!
//! The crate keeps the raw libiperf interaction small and explicit, while the
//! CLI layers Pushgateway export and option handling on top of the same code.

mod args;
#[doc(hidden)]
pub mod cli;
pub mod command;
mod help;

pub mod iperf;
pub mod metrics;
pub mod pushgateway;
pub mod version;

pub use command::{IperfCommand, IperfResult, RunningIperf};
pub use iperf::{Role, libiperf_version, usage_long};
pub use metrics::{
    MetricEvent, Metrics, MetricsMode, MetricsStream, WindowGaugeStats, WindowMetrics,
    aggregate_window,
};
pub use pushgateway::{PushGateway, PushGatewayConfig};
