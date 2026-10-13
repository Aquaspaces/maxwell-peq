use serde_json::{Map, Value};
use std::fmt;

/// Mirrors the exception classes the Python app distinguished. Every variant
/// reaches the page as `{"error": message}`; the variant only steers recovery.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Bad input or a refused operation (Python `ValueError`).
    Value(String),
    /// A worker reported failure; the map is its full JSON result.
    Worker(Map<String, Value>),
    /// I/O, timeouts, and anything else (Python `OSError`/`RuntimeError`).
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn value(message: impl Into<String>) -> Self {
        Error::Value(message.into())
    }

    pub fn other(message: impl Into<String>) -> Self {
        Error::Other(message.into())
    }

    /// A worker failure carrying only a message, as Python's `WorkerError(dict(error=...))`.
    pub fn worker(message: impl Into<String>) -> Self {
        let mut result = Map::new();
        result.insert("error".into(), Value::String(message.into()));
        Error::Worker(result)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Value(m) | Error::Other(m) => f.write_str(m),
            Error::Worker(result) => match result.get("error") {
                Some(Value::String(m)) => f.write_str(m),
                Some(other) => write!(f, "{other}"),
                None => f.write_str("Worker failed."),
            },
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Other(error.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::Value(error.to_string())
    }
}
