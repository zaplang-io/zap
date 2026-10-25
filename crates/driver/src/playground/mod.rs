//! `zap playground` — a local web playground over the shared `Session` engine
//! (I#238, design/0026-playground.md).
//!
//! A small `std::net` HTTP server serves a static editor page and a
//! `POST /run` endpoint. Because the WASM target is post-1.0, a snippet cannot
//! run in the browser; it is executed **server-side** through the REPL
//! [`Session`](crate::repl::Session) engine (I#159) — the same executor as
//! `zap run`/`zap repl`/doctests, so the playground has identical semantics.
//!
//! That makes the server a thing that runs arbitrary, untrusted native code, so
//! the sandbox is the heart of this ticket: **each request runs in its own
//! short-lived worker process** (`zap playground-worker`) under resource limits
//! (CPU, address space, file size, no core), with the parent enforcing a
//! wall-clock timeout, an output cap, and a cap on concurrent workers. A hard
//! fault (segfault/abort) in JIT'd code kills only the worker; the parent
//! reports it and the server stays up. One worker per request also means no two
//! requests ever share a heap, GC, or scheduler.
//!
//! True filesystem/network/syscall denial (an OS sandbox: seccomp / namespaces /
//! a container) is **deployment hardening**, called out as a gating follow-up
//! before the playground is ever exposed beyond loopback; the first cut binds to
//! `127.0.0.1` and must not be deployed publicly without it.
//!
//! Executing Zap needs the LLVM backend, so the playground is gated on the
//! `codegen` feature, exactly like `zap run`/`zap repl`; without it the verb
//! explains how to get a build that has it.

#[cfg(not(feature = "codegen"))]
use std::process::ExitCode;

#[cfg(feature = "codegen")]
mod sandbox;
#[cfg(feature = "codegen")]
mod server;
#[cfg(feature = "codegen")]
mod worker;

#[cfg(feature = "codegen")]
pub use sandbox::{handle_run, PlaygroundConfig, RunResult, Status};
#[cfg(feature = "codegen")]
pub use server::{serve, serve_on};
#[cfg(feature = "codegen")]
pub use worker::worker_main;

/// Run `zap playground` on a no-codegen build: the toolchain cannot execute
/// Zap, so the playground is unavailable — the same situation as `zap run`.
#[cfg(not(feature = "codegen"))]
pub fn serve(_host: String, _port: u16, _open: bool) -> ExitCode {
    eprintln!("zap playground: this build has no codegen, so it cannot execute Zap");
    eprintln!("(rebuild with --features zapc/llvm, which requires an LLVM toolchain)");
    ExitCode::from(1)
}

/// The hidden per-request worker verb is also unavailable without codegen.
#[cfg(not(feature = "codegen"))]
pub fn worker_main() -> ExitCode {
    eprintln!("zap playground-worker: this build has no codegen");
    ExitCode::from(1)
}
