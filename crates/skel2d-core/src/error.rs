use std::path::PathBuf;

use crate::validate::Issue;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("op {index}: {message}")]
    Op { index: usize, message: String },
    #[error("document is invalid ({} issue(s)): {}", .0.len(), .0.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<Issue>),
}

impl Error {
    pub fn op(index: usize, message: impl Into<String>) -> Self {
        Error::Op { index, message: message.into() }
    }
}
