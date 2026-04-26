//! Error types returned by the public library API.

use std::error::Error as StdError;
use std::fmt;

/// Result type used by the public iperf3-rs library API.
pub type Result<T> = std::result::Result<T, Error>;

/// Broad category for an [`Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The supplied iperf or iperf3-rs argument was invalid.
    InvalidArgument,
    /// The requested metrics mode is not valid.
    InvalidMetricsMode,
    /// Upstream libiperf reported an error.
    Libiperf,
    /// Pushgateway configuration or delivery failed.
    PushGateway,
    /// The background iperf worker failed before producing a normal result.
    Worker,
    /// An internal synchronization or setup invariant failed.
    Internal,
}

/// Error returned by the public library API.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl Error {
    /// Create an error with a category and message.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    /// Return the broad error category.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Return the human-readable error message.
    pub fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidArgument, message)
    }

    pub(crate) fn invalid_metrics_mode(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidMetricsMode, message)
    }

    pub(crate) fn libiperf(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Libiperf, message)
    }

    pub(crate) fn pushgateway(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::PushGateway, message)
    }

    pub(crate) fn worker(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Worker, message)
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    pub(crate) fn with_source(
        kind: ErrorKind,
        message: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}
