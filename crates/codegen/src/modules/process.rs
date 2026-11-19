//! `process` module — runtime-symbol declarations (ticket #97, D-0056;
//! subprocess ergonomics #976, D-0237).
//!
//! Blocking capture: `zap_process_run` / `zap_process_check_run` /
//! `zap_process_run_shell` all share the signature
//! `(argv|cmd: ptr, env: ptr, cwd: ptr, timeout_s: f64, input: ptr,
//! capture_output: i64, merge_stderr: i64) -> ZapProc*`; three getters unpack
//! the handle into the `(int64, str, str)` tuple. See `os_fs.inc.c`.
//!
//! Streaming: `zap_popen_*` over the opaque `ZapPopen` handle (returned as
//! `bytes`). The reads/writes park on the netpoller (POSIX).

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let f = cg.ctx.f64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());

    // run / check_run / run_shell: (cmd, env, cwd, timeout_s, input,
    // capture_output, merge_stderr) -> ZapProc*.
    cg.declare_rt(
        "zap_process_run",
        &[
            p.into(),
            p.into(),
            p.into(),
            f.into(),
            p.into(),
            i.into(),
            i.into(),
        ],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_process_check_run",
        &[
            p.into(),
            p.into(),
            p.into(),
            f.into(),
            p.into(),
            i.into(),
            i.into(),
        ],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_process_run_shell",
        &[
            p.into(),
            p.into(),
            p.into(),
            f.into(),
            p.into(),
            i.into(),
            i.into(),
        ],
        Some(p.into()),
    );
    cg.declare_rt("zap_process_result_code", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_process_result_stdout", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_process_result_stderr", &[p.into()], Some(p.into()));

    // Streaming Popen handle: a stable non-GC int64 address (not a GC pointer),
    // so every entry point takes/returns the handle as `i64`.
    cg.declare_rt(
        "zap_popen_spawn",
        &[p.into(), p.into(), p.into(), i.into()],
        Some(i.into()),
    );
    cg.declare_rt("zap_popen_write", &[i.into(), p.into()], Some(i.into()));
    cg.declare_rt("zap_popen_close_stdin", &[i.into()], None);
    cg.declare_rt(
        "zap_popen_read",
        &[i.into(), i.into(), i.into()],
        Some(p.into()),
    );
    cg.declare_rt("zap_popen_read_all", &[i.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_popen_poll", &[i.into()], Some(i.into()));
    cg.declare_rt("zap_popen_wait", &[i.into()], Some(i.into()));
    cg.declare_rt("zap_popen_returncode", &[i.into()], Some(i.into()));
    cg.declare_rt("zap_popen_pid", &[i.into()], Some(i.into()));
    cg.declare_rt("zap_popen_signal", &[i.into(), i.into()], None);
}
