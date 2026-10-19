//! The Zap package manager & project environment (issue #69, D-0052).
//!
//! This crate implements, independently of the compiler and LLVM:
//!
//! - [`manifest`] — the author-edited `zap.toml` (metadata + dependency sources).
//! - [`lockfile`] — the generated, committed `zap.lock` (resolved build list).
//! - [`resolver`] — Go-style Minimal Version Selection over semver versions,
//!   with a major version as the compatibility boundary.
//! - [`checksum`] — content integrity (`zap1:` SHA-256 tree hashes).
//! - [`zenv`] — the env-var-activated project environment (Zap module cache +
//!   nested Python venv).
//! - [`ops`] — the filesystem glue behind `zap add`/`zap remove`/`zap zenv`,
//!   which the `zap` CLI calls.
//!
//! The design is specified in `design/0009-packaging.md`.

pub mod checksum;
pub mod error;
pub mod lockfile;
pub mod manifest;
pub mod ops;
pub mod resolver;
pub mod zenv;

pub use error::{Error, Result};
pub use manifest::{Dependency, Manifest, Package, Source};
