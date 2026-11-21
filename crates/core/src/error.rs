//! Library error type.

use std::path::PathBuf;

/// Errors returned by the core library. Per-entry problems during a scan (unreadable
/// directories, failed git commands) are not errors: they are recorded in the result.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A scan root does not exist or cannot be read.
    #[error("cannot read scan root {path}: {source}")]
    Root {
        path: PathBuf,
        source: std::io::Error,
    },
    /// No scan roots were given.
    #[error("no scan roots given")]
    NoRoots,
    /// Reading or writing a file failed.
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// JSON could not be parsed or written.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The rules file is invalid.
    #[error("invalid rules: {0}")]
    Rules(String),
    /// The config file is missing (when named explicitly) or malformed.
    #[error("config file {path}: {message}")]
    Config { path: PathBuf, message: String },
    /// An `--active` path does not exist or matches nothing that was scanned.
    #[error("--active {path}: {message}")]
    Active { path: PathBuf, message: String },
    /// The scan file parses but is not a consistent document (an index out of range, a cycle,
    /// counts that overflow).
    #[error("invalid scan file: {field}: {message}")]
    InvalidScan { field: String, message: String },
    /// The scan file was written with a schema this build does not understand.
    #[error("scan file has schema_version {found}, this build reads {expected}")]
    Schema { found: u32, expected: u32 },
}

/// Library result type.
pub type Result<T> = std::result::Result<T, Error>;
