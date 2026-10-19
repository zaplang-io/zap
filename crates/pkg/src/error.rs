//! The package-manager error type.

use std::fmt;
use std::io;

/// A package-manager operation result.
pub type Result<T> = std::result::Result<T, Error>;

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
}

impl Error {
    /// An I/O error annotated with the offending path.
    pub fn io(path: impl Into<String>, source: io::Error) -> Error {
        Error::Io {
            path: path.into(),
            source,
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
