//! Typed application errors with documented exit codes.

use std::fmt;
use std::io;

/// A fatal error on the CLI path.
#[derive(Debug)]
pub enum AppError {
    /// An I/O operation failed. Context says which one.
    Io {
        /// What was being attempted.
        what: String,
        /// The underlying error.
        source: io::Error,
    },
    /// The config file is unreadable or invalid.
    Config(String),
    /// Diagnostics could not be configured or installed.
    Logging(String),
    /// The terminal could not be managed.
    Tui(io::Error),
}

impl AppError {
    /// Wraps an I/O error with the operation that failed.
    pub fn io(what: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            what: what.into(),
            source,
        }
    }

    /// Exit code: 2 for configuration/usage problems, 1 for runtime failures.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Config(_) | Self::Logging(_) => 2,
            Self::Io { .. } | Self::Tui(_) => 1,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { what, source } => write!(f, "{what}: {source}"),
            Self::Config(message) => write!(f, "config error: {message}"),
            Self::Logging(message) => write!(f, "logging error: {message}"),
            Self::Tui(error) => write!(f, "terminal error: {error}"),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Tui(error) => Some(error),
            Self::Config(_) | Self::Logging(_) => None,
        }
    }
}
