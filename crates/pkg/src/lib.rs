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
//! - [`index`] / [`archive`] / [`registry`] — the static-file registry read
//!   path (design/0027-registry.md, I#236).
//! - [`publish`] / [`discovery`] — the `zap publish` write path and
//!   `zap search`/`zap info` discovery (design/0027-registry.md, I#237).
//!
//! The design is specified in `design/0009-packaging.md` and, for the registry,
//! `design/0027-registry.md`.

pub mod archive;
pub mod checksum;
pub mod discovery;
pub mod error;
pub mod index;
pub mod lockfile;
pub mod manifest;
pub mod name;
pub mod ops;
pub mod publish;
pub mod registry;
pub mod resolver;
pub mod zenv;

pub use discovery::PackageSummary;
pub use error::{Error, Result, Warning};
pub use manifest::{Dependency, Manifest, Package, RegistryDef, Source};
pub use name::RegistryName;
pub use publish::Published;
