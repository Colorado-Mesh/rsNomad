use thiserror::Error;

#[derive(Debug, Error)]
pub enum NomadError {
    #[error("{0}")]
    Message(String),
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("path traversal rejected")]
    PathTraversal,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("response too large ({size} > {max})")]
    TooLarge { size: usize, max: usize },
    #[error("serving is not running")]
    NotRunning,
    #[error("already running")]
    AlreadyRunning,
}

impl NomadError {
    pub fn message(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }
}
