//! The package-manager error type.

use std::fmt;
use std::io;

pub use zapc_diag::ErrorCode;

/// A package-manager operation result.
pub type Result<T> = std::result::Result<T, Error>;

/// A non-fatal registry diagnostic (e.g. a yanked-version selection), carrying
/// a stable `ZAPnnnn` code (D-0071). Collected during resolution and surfaced
/// by the CLI so a build is not silently affected. Errors use [`Error`]; this
/// is the warning channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub code: ErrorCode,
    pub message: String,
}

impl Warning {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Warning {
        Warning {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "warning[{}]: {}", self.code, self.message)
    }
}

/// Everything that can go wrong parsing manifests/locks, resolving versions, or
/// touching the filesystem. Kept as one flat enum with a clear `Display` so the
/// CLI can print `zap: <error>` without further wrapping.
#[derive(Debug)]
pub enum Error {
    /// A malformed or inconsistent `zap.toml`.
    Manifest(String),
    /// A malformed `zap.lock`.
    Lockfile(String),
    /// Version parsing / resolution failure.
    Resolve(String),
    /// A `zenv` operation failed.
    Zenv(String),
    /// A filesystem error, with the path that triggered it.
    Io { path: String, source: io::Error },
    /// A registry-client failure, carrying a stable `ZAPnnnn` code (D-0071):
    /// checksum/archive mismatch, `min_zap` too high, name conflict, or an
    /// index/transport failure.
    Registry { code: ErrorCode, message: String },
}

impl Error {
    /// An I/O error annotated with the offending path.
    pub fn io(path: impl Into<String>, source: io::Error) -> Error {
        Error::Io {
            path: path.into(),
            source,
        }
    }

    /// A registry error with a stable diagnostic code.
    pub fn registry(code: ErrorCode, message: impl Into<String>) -> Error {
        Error::Registry {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Manifest(m) => write!(f, "zap.toml: {m}"),
            Error::Lockfile(m) => write!(f, "zap.lock: {m}"),
            Error::Resolve(m) => write!(f, "dependency resolution: {m}"),
            Error::Zenv(m) => write!(f, "zenv: {m}"),
            Error::Io { path, source } => write!(f, "{path}: {source}"),
            Error::Registry { code, message } => write!(f, "error[{code}]: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
