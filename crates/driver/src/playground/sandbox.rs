//! The sandbox: run one untrusted snippet in an isolated worker process under
//! resource limits, with a parent-enforced wall-clock timeout and output cap.
//!
//! Isolation model (design/0026-playground.md §The sandbox): the `Session`
//! engine is in-process MCJIT with a process-global GC and scheduler, and a hard
//! fault in JIT'd code ends the process — so a shared, long-lived `Session`
//! cannot be the target for untrusted input. Instead the parent spawns a fresh
//! `zap playground-worker` per request (its own heap/JIT/GC/scheduler), passes
//! the snippet on stdin, and reads one JSON result from its stdout. A worker
//! that segfaults/aborts kills only itself; the parent reports `crashed` and
//! stays up. Two concurrent requests are two processes with nothing shared.
//!
//! What this enforces (first cut): process isolation, hard-fault containment,
//! a wall-clock timeout, `RLIMIT_CPU`/`RLIMIT_AS`/`RLIMIT_FSIZE`/`RLIMIT_CORE`
//! in the worker, and an output byte cap. What it defers to an OS sandbox
//! (deployment hardening): true filesystem/network/syscall denial.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::json::{self, Json};

/// Environment keys the parent uses to pass resource limits to the worker. The
/// worker reads them in [`crate::playground::worker`]; defaults apply if unset.
pub(crate) const ENV_CPU_SECS: &str = "ZAP_PG_CPU_SECS";
pub(crate) const ENV_AS_BYTES: &str = "ZAP_PG_AS_BYTES";
pub(crate) const ENV_FSIZE_BYTES: &str = "ZAP_PG_FSIZE_BYTES";
pub(crate) const ENV_OUTPUT_CAP: &str = "ZAP_PG_OUTPUT_CAP";
pub(crate) const ENV_RUN_SECS: &str = "ZAP_PG_RUN_SECS";

/// Extra wall-clock the parent allows on top of the user-code budget to cover
/// worker startup (the C runtime is compiled and the JIT initialized per
/// process). The worker bounds the *snippet* with `alarm`, so this only guards
/// against a worker that never starts running at all.
const INIT_GRACE: Duration = Duration::from_secs(60);

/// Resource limits and routing for the playground. Cloneable so each connection
/// thread gets its own copy.
#[derive(Clone, Debug)]
pub struct PlaygroundConfig {
    /// The `zap` binary to spawn the worker from (`<bin> playground-worker`).
    /// Defaults to the running executable; tests point it at the test binary.
    pub worker_bin: PathBuf,
    /// Wall-clock budget for the *snippet* (user code). The worker arms an
    /// `alarm` for this many seconds right before evaluating, so startup cost
    /// does not count against it; the parent adds [`INIT_GRACE`] on top as a
    /// backstop for a worker that never begins running.
    pub timeout: Duration,
    /// `RLIMIT_CPU` backstop in the worker (seconds). A busy loop that somehow
    /// escapes the wall-clock still dies here.
    pub cpu_secs: u64,
    /// `RLIMIT_AS` in the worker (bytes): a cap on total address space. Set
    /// generously — MCJIT/LLVM need headroom — and tuned in the container.
    pub mem_bytes: u64,
    /// Maximum bytes of captured snippet output returned to the browser; extra
    /// is dropped and `truncated` is set.
    pub output_cap: usize,
    /// Maximum number of workers running at once (the server rejects excess
    /// with HTTP 429).
    pub max_concurrent: usize,
}

impl Default for PlaygroundConfig {
    fn default() -> Self {
        Self {
            worker_bin: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("zap")),
            timeout: Duration::from_secs(10),
            cpu_secs: 15, // a backstop above the 10s wall-clock, not the main gate
            mem_bytes: 1024 * 1024 * 1024, // 1 GiB
            output_cap: 64 * 1024, // 64 KiB
            max_concurrent: 4,
        }
    }
}

/// The top-level disposition of a run, as shown to the browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Compiled and ran without error.
    Ok,
    /// Rejected at compile time (one or more `ZAPnnnn` diagnostics).
    Error,
    /// Compiled, but raised an uncaught exception (contained by the engine).
    RuntimeError,
    /// Killed for exceeding the wall-clock or CPU budget.
    Timeout,
    /// The worker exceeded the output/file-size limit (`SIGXFSZ`).
    OutputLimit,
    /// A hard fault (segfault/abort/…) ended the worker; contained here.
    Crashed,
    /// The worker could not be run or produced no parseable result.
    Internal,
}

impl Status {
    /// The lowercase tag used in the JSON response and in tests.
    pub fn tag(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Error => "error",
            Status::RuntimeError => "runtime_error",
            Status::Timeout => "timeout",
            Status::OutputLimit => "output_limit",
            Status::Crashed => "crashed",
            Status::Internal => "internal",
        }
    }
}

/// The result of running one snippet.
#[derive(Clone, Debug)]
pub struct RunResult {
    pub status: Status,
    /// Captured standard output (possibly truncated to `output_cap`).
    pub stdout: String,
    /// Formatted `error[ZAPnnnn]: ...` compile diagnostics.
    pub diagnostics: Vec<String>,
    /// The message of an uncaught runtime exception, if any.
    pub runtime_error: Option<String>,
    /// True if `stdout` was truncated at the cap.
    pub truncated: bool,
    /// A human note for the non-program statuses (timeout/crashed/internal).
    pub message: Option<String>,
}

impl RunResult {
    fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: Status::Internal,
            stdout: String::new(),
            diagnostics: Vec::new(),
            runtime_error: None,
            truncated: false,
            message: Some(msg.into()),
        }
    }

    /// Serialize to the JSON the browser consumes.
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("status", Json::str(self.status.tag())),
            ("stdout", Json::str(self.stdout.clone())),
            (
                "diagnostics",
                Json::Arr(
                    self.diagnostics
                        .iter()
                        .map(|d| Json::str(d.as_str()))
                        .collect(),
                ),
            ),
            (
                "runtime_error",
                match &self.runtime_error {
                    Some(e) => Json::str(e.clone()),
                    None => Json::Null,
                },
            ),
            ("truncated", Json::Bool(self.truncated)),
            (
                "message",
                match &self.message {
                    Some(m) => Json::str(m.clone()),
                    None => Json::Null,
                },
            ),
        ])
    }
}

/// Run one snippet in an isolated worker process and return its result. Never
/// panics on untrusted input: every failure mode maps to a [`Status`].
pub fn handle_run(code: &str, config: &PlaygroundConfig) -> RunResult {
    let mut cmd = Command::new(&config.worker_bin);
    cmd.arg("playground-worker")
        .env(ENV_CPU_SECS, config.cpu_secs.to_string())
        .env(ENV_AS_BYTES, config.mem_bytes.to_string())
        // Give the file-size limit headroom above the output cap so normal large
        // output is captured then truncated gracefully; only a true runaway hits
        // the hard `SIGXFSZ` stop.
        .env(
            ENV_FSIZE_BYTES,
            (config.output_cap as u64 + 8 * 1024 * 1024).to_string(),
        )
        .env(ENV_OUTPUT_CAP, config.output_cap.to_string())
        .env(ENV_RUN_SECS, config.timeout.as_secs().max(1).to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // Put the worker in its own process group so a timeout kill reaps anything
    // it spawned, not just the worker itself.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            // SAFETY: async-signal-safe; `setpgid(0, 0)` makes the child a group
            // leader with pgid == its pid.
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return RunResult::internal(format!("failed to spawn worker: {e}")),
    };
    let pid = child.id();

    // Feed the snippet and close stdin so the worker's read-to-EOF returns.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(code.as_bytes());
        // dropping `stdin` closes it
    }

    // Drain stdout on a thread, capped, so a chatty worker can't make us block
    // or buffer without bound. The worker already truncates the snippet output
    // to `output_cap` before JSON-escaping it, so allow headroom for escaping
    // (worst case ~6x) plus framing so the legitimate result always fits.
    let cap = config.output_cap * 8 + 64 * 1024;
    let mut stdout = child.stdout.take();
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(out) = stdout.as_mut() {
            let mut chunk = [0u8; 8192];
            loop {
                match out.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        if buf.len() < cap {
                            buf.extend_from_slice(&chunk[..n.min(cap - buf.len())]);
                        }
                        // keep draining past the cap so the pipe doesn't block
                    }
                    Err(_) => break,
                }
            }
        }
        buf
    });

    // Wait for exit with a wall-clock deadline. A waiter thread reports the exit
    // status; `recv_timeout` is the deadline, with no extra dependency.
    let (tx, rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let status = child.wait();
        let _ = tx.send(status);
        child
    });

    // The worker's own `alarm` bounds the snippet; this parent deadline only
    // catches a worker that never starts running, so it includes startup grace.
    let timed_out;
    let exit = match rx.recv_timeout(config.timeout + INIT_GRACE) {
        Ok(status) => {
            timed_out = false;
            status.ok()
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            timed_out = true;
            kill_group(pid);
            // The waiter's `child.wait()` now returns; collect it so no zombie.
            let _ = rx.recv_timeout(Duration::from_secs(5));
            None
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            timed_out = false;
            None
        }
    };
    let _ = waiter.join();
    let out_bytes = reader.join().unwrap_or_default();

    interpret(timed_out, exit, &out_bytes, config)
}

/// Turn the worker's exit disposition + captured bytes into a [`RunResult`].
fn interpret(
    timed_out: bool,
    exit: Option<std::process::ExitStatus>,
    out_bytes: &[u8],
    config: &PlaygroundConfig,
) -> RunResult {
    if timed_out {
        return RunResult {
            status: Status::Timeout,
            stdout: String::new(),
            diagnostics: Vec::new(),
            runtime_error: None,
            truncated: false,
            message: Some(format!(
                "execution exceeded the {}s time limit",
                config.timeout.as_secs()
            )),
        };
    }

    let Some(status) = exit else {
        return RunResult::internal("worker did not report an exit status");
    };

    // A worker killed by a signal produced no result JSON: a hard fault, a CPU
    // backstop, or the file-size limit. Contain and classify it.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return RunResult {
                status: signal_status(sig),
                stdout: String::new(),
                diagnostics: Vec::new(),
                runtime_error: None,
                truncated: false,
                message: Some(signal_message(sig)),
            };
        }
    }

    if !status.success() {
        // The worker exited non-zero without a signal: treat any JSON it managed
        // to print as authoritative, else report an internal error.
        if out_bytes.is_empty() {
            return RunResult::internal("worker exited with an error and no output");
        }
    }

    parse_worker_json(out_bytes, config)
}

/// Map a terminating signal to a status.
#[cfg(unix)]
fn signal_status(sig: i32) -> Status {
    match sig {
        libc::SIGALRM | libc::SIGXCPU => Status::Timeout,
        libc::SIGXFSZ => Status::OutputLimit,
        _ => Status::Crashed,
    }
}

#[cfg(unix)]
fn signal_message(sig: i32) -> String {
    match sig {
        libc::SIGALRM => "execution exceeded the time limit".to_string(),
        libc::SIGXCPU => "execution exceeded the CPU time limit".to_string(),
        libc::SIGXFSZ => "output exceeded the size limit".to_string(),
        libc::SIGSEGV => "the program crashed (segmentation fault)".to_string(),
        libc::SIGABRT => "the program aborted".to_string(),
        libc::SIGBUS => "the program crashed (bus error)".to_string(),
        libc::SIGILL => "the program crashed (illegal instruction)".to_string(),
        libc::SIGFPE => "the program crashed (arithmetic error)".to_string(),
        other => format!("the program was terminated by signal {other}"),
    }
}

/// Parse the worker's JSON result `{stdout, diagnostics[], runtime_error,
/// truncated, internal?}` and derive the top-level status.
fn parse_worker_json(out_bytes: &[u8], config: &PlaygroundConfig) -> RunResult {
    let text = String::from_utf8_lossy(out_bytes);
    let value = match json::parse(text.trim()) {
        Ok(v) => v,
        Err(_) => return RunResult::internal("worker produced no parseable result"),
    };

    if let Some(internal) = value.get("internal").and_then(Json::as_str) {
        return RunResult::internal(internal.to_string());
    }

    let mut stdout = value
        .get("stdout")
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    let mut truncated = value
        .get("truncated")
        .map(|t| matches!(t, Json::Bool(true)))
        .unwrap_or(false);
    // Defensive second cap in the parent in case the worker misbehaves.
    if stdout.len() > config.output_cap {
        stdout.truncate(floor_char_boundary(&stdout, config.output_cap));
        truncated = true;
    }

    let diagnostics: Vec<String> = value
        .get("diagnostics")
        .and_then(Json::as_array)
        .map(|xs| {
            xs.iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let runtime_error = value
        .get("runtime_error")
        .and_then(Json::as_str)
        .map(str::to_string);

    let status = if !diagnostics.is_empty() {
        Status::Error
    } else if runtime_error.is_some() {
        Status::RuntimeError
    } else {
        Status::Ok
    };

    RunResult {
        status,
        stdout,
        diagnostics,
        runtime_error,
        truncated,
        message: None,
    }
}

/// Largest index `<= max` that lies on a UTF-8 char boundary (stable since
/// `str::floor_char_boundary` is still unstable).
fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// SIGKILL the worker's whole process group (it is a group leader with
/// pgid == pid); falls back to killing the pid on non-unix.
fn kill_group(pid: u32) {
    #[cfg(unix)]
    unsafe {
        // Negative pid targets the process group in `kill(2)`.
        libc::kill(-(pid as i32), libc::SIGKILL);
        libc::kill(pid as i32, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}
