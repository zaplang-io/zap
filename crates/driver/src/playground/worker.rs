//! The per-request worker: `zap playground-worker` (hidden verb).
//!
//! It reads one snippet from stdin, evaluates it through a fresh
//! [`Session`](crate::repl::Session), and prints a single JSON result to stdout
//! for the parent to relay. It is the unit of isolation: one process, one
//! snippet, then exit. Before running the snippet it lowers its own resource
//! limits (CPU/address-space/file-size/core) so a runaway cannot exhaust the
//! host; the parent sets the numeric limits via environment variables and
//! additionally enforces the wall-clock timeout and output cap.
//!
//! The snippet's own output goes to the process's fd 1 (the C runtime's
//! buffered `stdout`), so we capture fd 1 around the eval the same way the
//! doctest runner does, then restore it and print the result JSON. POSIX-only,
//! which matches the JIT.

use std::io::Read;
use std::process::ExitCode;

use crate::json::Json;
use crate::playground::sandbox::{
    ENV_AS_BYTES, ENV_CPU_SECS, ENV_FSIZE_BYTES, ENV_OUTPUT_CAP, ENV_RUN_SECS,
};
use crate::repl::Session;

/// Entry point for `zap playground-worker`.
pub fn worker_main() -> ExitCode {
    // Read the whole snippet from stdin.
    let mut code = String::new();
    if std::io::stdin().read_to_string(&mut code).is_err() {
        print_json(&Json::obj(vec![(
            "internal",
            Json::str("worker: failed to read snippet from stdin"),
        )]));
        return ExitCode::from(1);
    }

    let output_cap = env_usize(ENV_OUTPUT_CAP, 64 * 1024);

    // A CPU backstop can be set up front (it does not disturb engine init).
    set_rlimit(libc::RLIMIT_CPU, env_u64(ENV_CPU_SECS, 15));
    set_rlimit(libc::RLIMIT_CORE, 0);

    // Build the engine first so JIT/LLVM initialization is not constrained by the
    // address-space / file-size caps (a tight cap can otherwise break init).
    let mut session = match Session::new() {
        Ok(s) => s,
        Err(e) => {
            print_json(&Json::obj(vec![(
                "internal",
                Json::str(format!("worker: engine init failed: {e}")),
            )]));
            return ExitCode::from(1);
        }
    };

    // Now clamp address space and file size for the untrusted snippet.
    set_rlimit(libc::RLIMIT_AS, env_u64(ENV_AS_BYTES, 1024 * 1024 * 1024));
    set_rlimit(
        libc::RLIMIT_FSIZE,
        env_u64(ENV_FSIZE_BYTES, output_cap as u64 + 8 * 1024 * 1024),
    );

    // Bound the *snippet's* wall-clock with an alarm armed right here, so slow
    // startup (compiling the runtime, JIT init) never counts against it. A
    // runaway snippet gets `SIGALRM`, whose default action ends the worker; the
    // parent maps that to a timeout.
    // SAFETY: `alarm` is async-signal-safe and simply schedules `SIGALRM`.
    unsafe {
        libc::alarm(env_u64(ENV_RUN_SECS, 10) as libc::c_uint);
    }

    let (outcome, captured) = capture_stdout(|| session.eval(&code));

    // Completed in time — cancel the pending alarm.
    // SAFETY: as above.
    unsafe {
        libc::alarm(0);
    }

    let truncated = captured.len() > output_cap;
    let stdout = if truncated {
        captured[..floor_char_boundary(&captured, output_cap)].to_string()
    } else {
        captured
    };

    let result = Json::obj(vec![
        ("stdout", Json::str(stdout)),
        (
            "diagnostics",
            Json::Arr(
                outcome
                    .diagnostics
                    .iter()
                    .map(|d| Json::str(d.as_str()))
                    .collect(),
            ),
        ),
        (
            "runtime_error",
            match &outcome.runtime_error {
                Some(e) => Json::str(e.clone()),
                None => Json::Null,
            },
        ),
        ("truncated", Json::Bool(truncated)),
    ]);
    print_json(&result);
    ExitCode::SUCCESS
}

/// Print one JSON object to the real stdout (fd 1 is restored by this point).
fn print_json(value: &Json) {
    use std::io::Write;
    let s = value.to_string();
    let mut out = std::io::stdout();
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

/// Lower a soft resource limit to `value`, leaving the hard limit alone (we only
/// ever tighten). Best-effort: a failure is non-fatal.
fn set_rlimit(resource: libc::__rlimit_resource_t, value: u64) {
    // SAFETY: read the current limit, lower the soft limit, write it back.
    unsafe {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(resource, &mut lim) != 0 {
            return;
        }
        let cur = value as libc::rlim_t;
        lim.rlim_cur = if lim.rlim_max == libc::RLIM_INFINITY {
            cur
        } else {
            cur.min(lim.rlim_max)
        };
        libc::setrlimit(resource, &lim);
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Run `f`, returning its value plus everything it wrote to fd 1. The JIT'd
/// snippet prints through the C runtime's buffered `stdout`, so redirect fd 1 to
/// a temp file, flush the C streams, then restore and read it back. POSIX-only.
/// If the OS calls fail, `f` still runs and the capture degrades to empty.
///
/// (A near-copy of the doctest runner's helper; kept local so the fresh #159/
/// #234 code is untouched. A shared capture is a small follow-up.)
fn capture_stdout<T, F: FnOnce() -> T>(f: F) -> (T, String) {
    use std::io::{Read as _, Seek, SeekFrom, Write};
    use std::os::unix::io::AsRawFd;

    let _ = std::io::stdout().flush();

    let tmp_path = std::env::temp_dir().join(format!("zap-playground-{}", std::process::id()));
    let mut tmp = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp_path)
    {
        Ok(f) => f,
        Err(_) => return (f(), String::new()),
    };

    // SAFETY: plain POSIX fd juggling. `dup` the real stdout aside, point fd 1 at
    // the temp file for the duration of `f`, then restore it. `fflush(NULL)`
    // flushes every C output stream (the JIT runtime's `stdout` among them).
    let saved = unsafe {
        libc::fflush(std::ptr::null_mut());
        let saved = libc::dup(1);
        if saved < 0 {
            return (f(), String::new());
        }
        if libc::dup2(tmp.as_raw_fd(), 1) < 0 {
            libc::close(saved);
            return (f(), String::new());
        }
        saved
    };

    let value = f();

    let out = unsafe {
        libc::fflush(std::ptr::null_mut());
        libc::dup2(saved, 1);
        libc::close(saved);
        let mut s = String::new();
        let _ = tmp.seek(SeekFrom::Start(0));
        let _ = tmp.read_to_string(&mut s);
        s
    };
    drop(tmp);
    let _ = std::fs::remove_file(&tmp_path);
    (value, out)
}

/// Largest index `<= max` on a UTF-8 char boundary.
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
