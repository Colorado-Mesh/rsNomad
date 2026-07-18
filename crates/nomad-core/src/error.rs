//! Error types for Nomad page/file hosting.

use thiserror::Error;

/// Errors from Nomad path validation, content I/O, and node lifecycle.
#[derive(Debug, Error)]
pub enum NomadError {
    /// Generic message (lock poison, collisions, listing limits, etc.).
    #[error("{0}")]
    Message(String),
    /// Path string failed validation (empty, too deep, forbidden name, etc.).
    #[error("invalid path: {0}")]
    InvalidPath(String),
    /// Path escaped the content root or used a symlink / absolute component.
    #[error("path traversal rejected")]
    PathTraversal,
    /// Requested page or file does not exist under the content root.
    #[error("not found: {0}")]
    NotFound(String),
    /// Underlying filesystem or I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Page or file body exceeds the configured size cap.
    #[error("response too large ({size} > {max})")]
    TooLarge { size: usize, max: usize },
    /// Reserved for a future start/stop lifecycle API (unused in v0.1).
    #[error("serving is not running")]
    NotRunning,
    /// Reserved for a future start/stop lifecycle API (unused in v0.1).
    #[error("already running")]
    AlreadyRunning,
}

impl NomadError {
    /// Build a [`NomadError::Message`] from any string-like value.
    pub fn message(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }
}
