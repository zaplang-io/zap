//! `process` module — sema facet (ticket #97, D-0056; subprocess ergonomics
//! #976, D-0237).
//!
//! Blocking capture: `run` / `check_run` / `run_shell` spawn a child, feed an
//! optional `input`, capture stdout/stderr, wait (with an optional `timeout`
//! that SIGKILLs on expiry), and return a `(exit_code, stdout, stderr)` tuple.
//! `check_run` raises `RuntimeError` on a non-zero exit; `run_shell` runs its
//! `str` command through the system shell. The options (`env`/`cwd`/`timeout`/
//! `input`/`capture_output`/`merge_stderr`) are keyword-bound via the seam
//! registry (mirroring Python `subprocess.run`).
//!
//! Streaming: the prelude `Popen` struct (built by `process.Popen(...)` ⇒
//! `__process_popen`) drives these internal, positional-only `_popen_*`
//! primitives — spawn-with-pipes, incremental stdin write, non-blocking
//! stdout/stderr reads parked on the netpoller, `poll`/`wait`/`returncode`,
//! `terminate`/`kill`. The handle is a stable non-GC `int64` (the `http.Client`
//! pool pattern — safe to hold and mutate across a netpoller park). Streaming is
//! POSIX-only for now (D-0237).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, expect_argc_range, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "process",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // process.run(cmd: list[str], env={}, cwd="", timeout=0.0, input="",
        // capture_output=True, merge_stderr=False) -> (int64, str, str).
        // check_run is identical but raises RuntimeError on a non-zero exit;
        // run_shell takes a `str` command run through the system shell. The
        // options are keyword-bound (seam registry); `cmd` is the one positional.
        "run" | "check_run" | "run_shell" => {
            expect_argc_range(checker, "process", method, args, 1, 7, span);
            Ty::Tuple(vec![Ty::Int64, Ty::Str, Ty::Str])
        }
        // --- internal streaming primitives (prelude `Popen` only; positional) ---
        // _popen_spawn(argv: list[str], env: dict[str,str], cwd: str, flags: int64)
        // -> int64 (the opaque ZapPopen handle; a stable non-GC malloc address,
        // the `http.Client` pool pattern). `flags` bitpacks merge_stderr(1),
        // want_stdin(2), want_stdout(4), want_stderr(8).
        "_popen_spawn" => {
            expect_argc(checker, "process", method, args, 4, span);
            Ty::Int64
        }
        // _popen_write(h, data: str) -> int64 (bytes written == len(data)).
        "_popen_write" => {
            expect_argc(checker, "process", method, args, 2, span);
            Ty::Int64
        }
        // _popen_close_stdin(h).
        "_popen_close_stdin" => {
            expect_argc(checker, "process", method, args, 1, span);
            Ty::Unit
        }
        // _popen_read(h, which: int64, maxn: int64) -> bytes (which: 1=out, 2=err).
        "_popen_read" => {
            expect_argc(checker, "process", method, args, 3, span);
            Ty::Bytes
        }
        // _popen_read_all(h, which: int64) -> bytes.
        "_popen_read_all" => {
            expect_argc(checker, "process", method, args, 2, span);
            Ty::Bytes
        }
        // _popen_poll(h) -> int64 (exit status, or -256 while still running).
        "_popen_poll" => {
            expect_argc(checker, "process", method, args, 1, span);
            Ty::Int64
        }
        // _popen_wait(h) -> int64 (blocks, returns the exit status).
        "_popen_wait" => {
            expect_argc(checker, "process", method, args, 1, span);
            Ty::Int64
        }
        // _popen_returncode(h) -> int64 (raises RuntimeError if still running).
        "_popen_returncode" => {
            expect_argc(checker, "process", method, args, 1, span);
            Ty::Int64
        }
        // _popen_pid(h) -> int64.
        "_popen_pid" => {
            expect_argc(checker, "process", method, args, 1, span);
            Ty::Int64
        }
        // _popen_signal(h, sig: int64) — terminate(15) / kill(9).
        "_popen_signal" => {
            expect_argc(checker, "process", method, args, 2, span);
            Ty::Unit
        }
        _ => return None,
    })
}
