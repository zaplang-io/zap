//! Source-contract tests over the embedded C runtime TEXT (core runtime: GC,
//! scheduler, coroutines, netpoller, preemption) and the runtime-fragment manifest.
//! They inspect the private `RUNTIME_C` / `RUNTIME_GC_C` consts, so they live beside
//! codegen (same crate), not in `tests/` (which sees only the public API). Split out
//! of the `lib.rs` crate root (#1025). See crates/codegen/FOLDER_ARCHITECTURE.md.

use super::*;

/// Drift guard for the runtime-fragment manifest (host-only, no LLVM). Asserts
/// three ways that the on-disk `src/runtime/` tree, `RUNTIME_FRAGMENTS`, and the
/// `#include`s in `zaprt.c` all agree, so a fragment can never half-land and
/// fail only in full/arm64 CI or `zap test --doc`:
///   1. every file under `src/runtime/` has a manifest row (added a file,
///      forgot the manifest → embed/write/hash would silently skip it);
///   2. every `.inc.c` manifest row is `#include`d by `zaprt.c` (embedded but
///      never compiled into the TU);
///   3. every `#include "runtime/…"` in `zaprt.c` has a manifest row (compiled
///      but never written beside `zaprt.c` → a compile failure only where `cc`
///      actually runs).
///
/// Also checks every manifest text is non-empty.
#[test]
fn runtime_manifest_matches_disk_and_includes() {
    use std::collections::BTreeSet;

    let src_runtime = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/runtime");

    // On-disk fragment files, as manifest-style relative paths ("runtime/..").
    let mut on_disk: BTreeSet<String> = BTreeSet::new();
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut BTreeSet<String>) {
        for e in std::fs::read_dir(dir)
            .expect("read_dir src/runtime")
            .flatten()
        {
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(format!("runtime/{rel}"));
            }
        }
    }
    walk(&src_runtime, &src_runtime, &mut on_disk);

    let manifest_paths: BTreeSet<String> = runtime_manifest::RUNTIME_FRAGMENTS
        .iter()
        .map(|f| f.path.to_string())
        .collect();

    // (1) disk ⊆ manifest.
    for f in &on_disk {
        assert!(
            manifest_paths.contains(f),
            "src/{f} exists on disk but is missing from RUNTIME_FRAGMENTS \
             (add a row in runtime_manifest.rs + its include_str! const in lib.rs)"
        );
    }
    // Manifest `runtime/` rows ⊆ disk (catches a typo'd path or a deleted file).
    for p in &manifest_paths {
        if let Some(rel) = p.strip_prefix("runtime/") {
            assert!(
                src_runtime.join(rel).is_file(),
                "RUNTIME_FRAGMENTS lists runtime/{rel} but no such file exists on disk"
            );
        }
    }

    // (2) every `.inc.c` row is #included by zaprt.c; (3) every runtime/ include
    // in zaprt.c is a manifest row. Non-empty texts throughout.
    for f in runtime_manifest::RUNTIME_FRAGMENTS {
        assert!(
            !f.text.is_empty(),
            "runtime fragment {} is embedded empty",
            f.path
        );
        if f.path.ends_with(".inc.c") {
            let needle = format!("#include \"{}\"", f.path);
            assert!(
                RUNTIME_C.contains(&needle),
                "fragment {} is in the manifest but zaprt.c never `{needle}`s it",
                f.path
            );
        }
    }
    for line in RUNTIME_C.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#include \"runtime/") {
            if let Some(path) = rest.strip_suffix('"') {
                let full = format!("runtime/{path}");
                assert!(
                    manifest_paths.contains(&full),
                    "zaprt.c `#include`s {full} but it has no RUNTIME_FRAGMENTS row"
                );
            }
        }
    }
}

/// The default (non-LTO) runtime compile is `-O2` on EVERY target (the
/// aarch64 gate from #868 is gone now that I#869 is fixed in the runtime);
/// `--debug` lowers it to `-O0`. This pins the exact flags the link step
/// passes to `cc`, independent of a working toolchain (so it runs everywhere
/// codegen builds, not just where `objdump` and a C compiler are present — see
/// `crates/driver/tests/runtime_opt.rs` for the behavioural disassembly guard).
#[test]
fn runtime_opt_flag_o2_by_default_o0_with_debug() {
    assert_eq!(
        runtime_opt_flag(false),
        "-O2",
        "the default `zap build` must compile the C runtime at `-O2` on every target \
         (aarch64 included, I#869)"
    );
    assert_eq!(
        runtime_opt_flag(true),
        "-O0",
        "`zap build --debug` must compile the C runtime at `-O0`"
    );
}

/// Remove `/* ... */` and `// ...` comments from a C snippet, so a source-contract
/// test inspects CODE only (the explanatory comments legitimately name `cur_m`).
fn strip_c_comments(c: &str) -> String {
    let mut out = String::with_capacity(c.len());
    let b = c.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// I#869 tripwire. The scheduler resumes a coroutine on whichever worker M
/// picks it up, so a coroutine can PARK on one OS thread and finish on
/// another. `cur_m` is `__thread`: on aarch64, `-O2` GCC/clang materialise a
/// thread-local's address from `tpidr_el0` once and keep it in a callee-saved
/// register for the whole function, and `swapcontext` restores that register
/// on resume — so any function that reads `cur_m`, runs code that can park,
/// then "re-reads" `cur_m` silently reads the OLD thread's slot. In
/// `g_bootstrap` that made a finishing coroutine swap into another thread's
/// scheduler context (SIGSEGV / stack smashing). x86_64 executables happen to
/// reload `%fs:cur_m` every time, so only an aarch64 (or `-fPIC` x86) build
/// shows it; this pins the source-level contract on every host: after the
/// coroutine body (`g_entry_fn(g)`) returns, `g_bootstrap` must not touch the
/// `cur_m` thread-local again — it takes the resuming M from the `G`
/// (`g->run_m`, set by `switch_to_g`), which is not TLS-addressed.
#[test]
fn g_bootstrap_does_not_reread_cur_m_after_the_coroutine_body() {
    let src = RUNTIME_C;
    let start = src
        .find("static void g_bootstrap(void) {")
        .expect("g_bootstrap must exist in zaprt.c");
    let body_end = start + src[start..].find("\n}\n").expect("g_bootstrap must end");
    let body = strip_c_comments(&src[start..body_end]);
    let call = body
        .find("g_entry_fn(g);")
        .expect("g_bootstrap must run the coroutine body via g_entry_fn(g)");
    let after = &body[call + "g_entry_fn(g);".len()..];
    assert!(
        !after.contains("cur_m"),
        "I#869: g_bootstrap re-reads the thread-local `cur_m` after the coroutine \
         body returned. The body may have parked and resumed on another OS thread, \
         and at -O2 on aarch64 the compiler reuses the OLD thread's `&cur_m` — read \
         the resuming M from the G (`g->run_m`) instead. Offending tail:\n{after}"
    );
}

/// Replace every `/* ... */` and `// ...` comment in a C snippet with spaces of
/// the SAME length, so byte offsets into the result match the original source.
fn blank_c_comments(c: &str) -> String {
    let b = c.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let start = i;
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            for o in &mut out[start..i] {
                if *o != b'\n' {
                    *o = b' ';
                }
            }
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            let start = i;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            for o in &mut out[start..i] {
                *o = b' ';
            }
        } else {
            i += 1;
        }
    }
    String::from_utf8(out).expect("blanking ASCII comment bytes keeps valid UTF-8")
}

/// The `[start, end)` byte ranges of the runtime's guarded libgc layer: the
/// blocks between `ZAP_GC_GUARDED_LAYER_BEGIN` and `ZAP_GC_GUARDED_LAYER_END`.
fn guarded_layer_ranges(src: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(b) = src[from..].find("ZAP_GC_GUARDED_LAYER_BEGIN") {
        let b = from + b;
        let e = src[b..]
            .find("ZAP_GC_GUARDED_LAYER_END")
            .expect("every ZAP_GC_GUARDED_LAYER_BEGIN needs a matching _END")
            + b;
        ranges.push((b, e));
        from = e;
    }
    ranges
}

/// I#888 tripwire. Under Boehm the allocator lock is a plain non-recursive
/// mutex, and the async-preemption handler may switch a coroutine away at any
/// instruction unless it is inside a no-preempt window. A coroutine preempted
/// while inside libgc keeps the lock across the switch, and the next coroutine
/// on the same OS thread then self-deadlocks in its first allocation. So every
/// libgc entry point that takes the lock must sit inside the runtime's guarded
/// layer (`ZAP_GC_GUARDED_LAYER_BEGIN/END` in zaprt.c), bracketed by
/// `zap_np_enter()` / `zap_np_leave()` — and nothing else may name the raw
/// calls. `gc.collect()` (`GC_gcollect`, which holds the lock for the whole
/// collection) was the unguarded one that hung `gc_switch_alloc_stress`.
#[test]
fn raw_libgc_calls_appear_only_inside_the_guarded_layer() {
    let src = RUNTIME_C;
    let blanked = blank_c_comments(src);
    let layers = guarded_layer_ranges(src);
    assert!(
        !layers.is_empty(),
        "I#888: zaprt.c must declare its guarded libgc layer (ZAP_GC_GUARDED_LAYER_BEGIN/END)"
    );
    let raw = [
        "GC_gcollect(",
        "GC_register_finalizer(",
        "GC_get_heap_size(",
        "GC_get_free_bytes(",
        "GC_MALLOC(",
        "GC_MALLOC_ATOMIC(",
        "GC_pthread_create(",
        "GC_pthread_join(",
        "GC_pthread_detach(",
        "pthread_create(",
        "pthread_detach(",
    ];
    let mut offenders = Vec::new();
    for tok in raw {
        let mut from = 0;
        while let Some(i) = blanked[from..].find(tok) {
            let at = from + i;
            from = at + tok.len();
            // Whole-identifier match only (`zap_gc_pthread_create(` is the guarded wrapper).
            let prev = blanked[..at].chars().next_back().unwrap_or(' ');
            if prev.is_ascii_alphanumeric() || prev == '_' {
                continue;
            }
            // `#define GC_pthread_create pthread_create` style shims carry no call.
            let line_start = blanked[..at].rfind('\n').map_or(0, |n| n + 1);
            if blanked[line_start..at].trim_start().starts_with('#') {
                continue;
            }
            if !layers.iter().any(|&(b, e)| at >= b && at < e) {
                let line = src[..at].matches('\n').count() + 1;
                offenders.push(format!("zaprt.c:{line}: `{tok}`"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "I#888: raw libgc / thread-create calls outside the guarded layer (they take the \
         allocator lock without a no-preempt window, so a coroutine preempted inside one \
         deadlocks the next allocation on its M). Route them through the zap_gc_* wrappers:\n{}",
        offenders.join("\n")
    );
}

/// The other half of I#888: inside the guarded layer, every function that
/// makes a raw call must open the no-preempt window BEFORE it and close it
/// AFTER it.
#[test]
fn guarded_layer_wraps_every_raw_call_in_a_no_preempt_window() {
    let src = RUNTIME_C;
    let blanked = blank_c_comments(src);
    let raw = [
        "GC_gcollect(",
        "GC_register_finalizer(",
        "GC_get_heap_size(",
        "GC_get_free_bytes(",
        "GC_MALLOC(",
        "GC_MALLOC_ATOMIC(",
        "GC_pthread_create(",
        "GC_pthread_join(",
        "GC_pthread_detach(",
    ];
    let mut checked = 0;
    for (b, e) in guarded_layer_ranges(src) {
        let layer = &blanked[b..e];
        // Each layer function is `static inline ... { ... }` closed by a column-0 `}`.
        for func in layer.split("static inline").skip(1) {
            let body = &func[..func.find("\n}").unwrap_or(func.len())];
            for tok in raw {
                if let Some(at) = body.find(tok) {
                    let enter = body.find("zap_np_begin(np);");
                    let leave = body.rfind("zap_np_end(np);");
                    assert!(
                        enter.is_some_and(|x| x < at) && leave.is_some_and(|x| x > at),
                        "I#888: guarded-layer function calling `{tok}` must wrap it in \
                         zap_np_begin(np); ... zap_np_end(np); (np = zap_np_slot())\n{body}"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(
        checked >= 9,
        "I#888: expected the guarded layer to wrap all 9 raw libgc entry points, saw {checked}"
    );
}

/// Body (comments stripped) of the C function whose definition starts with `sig`.
fn c_fn_body(sig: &str) -> String {
    let start = RUNTIME_C
        .find(sig)
        .unwrap_or_else(|| panic!("zaprt.c must define `{sig}`"));
    let end = start + RUNTIME_C[start..].find("\n}\n").expect("function must end");
    strip_c_comments(&RUNTIME_C[start..end])
}

/// I#889 tripwire. A coroutine that switches away publishes WHY in `g->state`
/// (`G_DEAD` / `G_PARKED` / `G_RUNNABLE`) and then calls `zap_ctx_swap`;
/// `switch_to_g` dispatches on that state when the swap returns. The span from
/// the state store to the actual switch is microseconds (PLT stub, glibc's
/// context save, an `rt_sigprocmask` syscall), and a Boehm async preemption
/// landing in it rewrites the state to `G_RUNNABLE`, so the resumed sequence
/// later swaps out with `G_RUNNING` — a state the scheduler cannot dispatch, and
/// the coroutine is lost (hang at join / a parkmu that is never released). So at
/// every swap site the no-preempt window must be raised BEFORE the state store
/// (`g_enter_switch_out`), and must NOT be cleared coroutine-side before the swap
/// (the scheduler clears it after the swap returns).
#[test]
fn every_switch_out_site_is_a_no_preempt_window_until_the_swap() {
    for sig in [
        "static void g_bootstrap(void) {",
        "static void cosched(void) {",
        "static void copreempt(void) {",
        "static void copark(pthread_mutex_t *mu, Waiter *w) {",
    ] {
        let body = c_fn_body(sig);
        let swap = body
            .find("zap_ctx_swap(&g->ctx, &m->sched_ctx)")
            .unwrap_or_else(|| panic!("{sig} must swap back to the scheduler"));
        let enter = body
            .find("g_enter_switch_out(g);")
            .unwrap_or_else(|| panic!("I#889: {sig} must call g_enter_switch_out(g)"));
        let state = body
            .find("g->state = G_")
            .unwrap_or_else(|| panic!("{sig} must publish its scheduler state"));
        assert!(
            enter < state && state < swap,
            "I#889: in {sig} the no-preempt window must open BEFORE the `g->state` store \
             and the store must precede the swap:\n{body}"
        );
        let span = &body[enter..swap];
        for clears in ["no_preempt = 0", "zap_np_leave()", "no_preempt--"] {
            assert!(
                !span.contains(clears),
                "I#889: {sig} must not end the no-preempt window (`{clears}`) before the \
                 swap; switch_to_g ends it once the coroutine is stopped:\n{span}"
            );
        }
    }
}

/// The companion half of I#889: the scheduler ends the window the coroutine
/// opened, right after the swap back returns (the coroutine is stopped then).
#[test]
fn switch_to_g_ends_the_switch_out_window_after_the_swap() {
    let body = c_fn_body("static void switch_to_g(M *m, G *g) {");
    let swap = body
        .find("zap_ctx_swap(&m->sched_ctx, &g->ctx)")
        .expect("switch_to_g must swap the coroutine in");
    let clear = body[swap..]
        .find("g->no_preempt = 0;")
        .expect("I#889: switch_to_g must clear g->no_preempt after the swap returns");
    // ...and before it dispatches on the state the coroutine published.
    let dispatch = body[swap..]
        .find("g->state == G_PARKED")
        .expect("switch_to_g dispatches on g->state");
    assert!(
        clear < dispatch,
        "I#889: g->no_preempt must be cleared before switch_to_g dispatches on g->state"
    );
}

/// All runtime thread-local DECLARATIONS (comments stripped), as `name` per
/// `__thread` variable, across the runtime and the Immix collector source.
fn thread_local_names() -> Vec<String> {
    let mut names = Vec::new();
    for src in [RUNTIME_C, RUNTIME_GC_C] {
        for line in strip_c_comments(src).lines() {
            let t = line.trim();
            if !t.contains("__thread") || t.starts_with('#') {
                continue;
            }
            // `[extern] [static] __thread T *name [= init];` -> `name`
            let decl = t.split('=').next().unwrap().trim_end_matches(';').trim();
            if let Some(name) = decl
                .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .next()
            {
                if !name.is_empty() {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

/// I#891 tripwire (the stale-TLS class of I#869). A coroutine is resumed on
/// whichever OS thread picks it up; on aarch64 at `-O2` the compiler computes a
/// thread-local's ADDRESS once per function and holds it in a callee-saved
/// register that `swapcontext` restores unchanged, so any access after a park
/// (including the inlined `zap_np_lock`/`zap_np_unlock` in `chan_block`, and
/// `zap_self` in `zap_safepoint` after its cooperative preempt) touches the
/// thread the coroutine LEFT. Every runtime thread-local that coroutine code can
/// reach is therefore private (`*_tls`) and accessed only through a `noinline`
/// accessor with a volatile asm barrier (so neither GCC's nor LLVM's IPA can infer
/// it const/pure and merge calls), recomputing the thread pointer on every use.
/// A NEW `__thread` variable must be added here deliberately.
#[test]
fn runtime_thread_locals_are_reached_only_through_fresh_accessors() {
    let allowed_private = [
        (
            "zap_cur_m_tls",
            "zap_cur_m_slot",
            "#define cur_m (*zap_cur_m_slot())",
        ),
        (
            "zap_self_tls",
            "zap_self_slot",
            "#define zap_self (*zap_self_slot())",
        ),
        (
            "zap_shadow_head_tls",
            "zap_shadow_head_addr",
            "#define zap_shadow_head (*zap_shadow_head_addr())",
        ),
    ];
    // REPL-only catch state: set/read on one thread inside a single JIT call
    // (setjmp/longjmp), never across a coroutine switch.
    let allowed_other = ["zap_repl_catch", "zap_repl_exc"];
    let both = format!(
        "{}\n{}",
        strip_c_comments(RUNTIME_C),
        strip_c_comments(RUNTIME_GC_C)
    );
    for name in thread_local_names() {
        assert!(
            allowed_private.iter().any(|(n, _, _)| *n == name) || allowed_other.contains(&name.as_str()),
            "I#891: new runtime `__thread` variable `{name}`: a coroutine can migrate OS threads, so \
             it must be reached only via a noinline accessor (like zap_cur_m_slot) — add it to this \
             test deliberately"
        );
    }
    for (tls, accessor, macro_def) in allowed_private {
        // The DEFINITION (`name(void) {`), not the forward declaration.
        let at = both
            .find(&format!("{accessor}(void) {{"))
            .unwrap_or_else(|| panic!("I#891: accessor `{accessor}` must be defined"));
        let head = &both[at.saturating_sub(120)..at + 260];
        assert!(
            head.contains("noinline"),
            "I#891: `{accessor}` must be `__attribute__((noinline))`:\n{head}"
        );
        assert!(
            head.contains("__asm__ __volatile__"),
            "I#891: `{accessor}` must contain a volatile asm so it is never inferred const/pure:\n{head}"
        );
        assert!(
            head.contains(tls),
            "I#891: `{accessor}` must return the address of `{tls}`:\n{head}"
        );
        assert!(
            both.contains(macro_def),
            "I#891: expected `{macro_def}` so every use recomputes the thread pointer"
        );
    }
}

/// I#891: the no-preempt bookkeeping reads the current coroutine's counter fresh.
#[test]
fn np_bookkeeping_reads_the_thread_local_fresh_on_every_call() {
    let code = strip_c_comments(RUNTIME_C);
    let at = code
        .find("int *zap_np_slot(void) {")
        .expect("zap_np_slot must be defined");
    let head = &code[at.saturating_sub(80)..at + 320];
    assert!(
        head.contains("noinline") && head.contains("__asm__ __volatile__"),
        "{head}"
    );
    for f in [
        "static void zap_np_enter(void) {",
        "static void zap_np_leave(void) {",
    ] {
        let b = code.find(f).unwrap_or_else(|| panic!("{f} must exist"));
        assert!(
            code[b..b + 120].contains("zap_np_slot()"),
            "I#891: {f} must go through zap_np_slot() (fresh thread-local read)"
        );
    }
}

/// I#891 / I#223 defense in depth: the Boehm preemption handler must not
/// switch a coroutine away unless the interrupted PC is inside the program's
/// own text — not in libgc / libc / the loader, where the allocator lock and
/// other thread-affine state live.
#[test]
fn preempt_handler_requires_the_interrupted_pc_to_be_program_text() {
    let body = c_fn_body("static void zap_preempt_handler(int sig, siginfo_t *info, void *uc) {");
    assert!(
        body.contains("zap_pc_in_program_text(uc)"),
        "I#891: zap_preempt_handler must gate copreempt() on zap_pc_in_program_text(uc):\n{body}"
    );
    let gate = body.find("zap_pc_in_program_text(uc)").unwrap();
    let swap = body.find("copreempt();").expect("handler must copreempt");
    assert!(gate < swap, "the PC gate must precede the copreempt() call");
}

/// I#885 tripwire. Under Boehm a stopped thread is scanned as
/// `[sp, stack_end)`. `switch_to_g` used to publish the coroutine's stack top
/// as `stack_end` (`set_bottom(m, &g->sb)`) BEFORE the swap that moves `sp`
/// there, and restore the native bottom only AFTER swapping back, so a
/// stop-the-world in either gap saw `sp` and `stack_end` on different stacks:
/// a multi-MB cross-mapping range, and a mark thread faulted in an unmapped
/// hole. The fix never changes `stack_end` after the thread is registered
/// (it stays the native stack top) and instead corrects the recorded `sp` at
/// scan time (`GC_set_sp_corrector`), when the world is stopped and `sp` /
/// `stack_end` cannot be torn. So the switch path must not move the bottom.
#[test]
fn switch_to_g_does_not_move_the_boehm_stack_bottom() {
    let src = RUNTIME_C;
    let start = src
        .find("static void switch_to_g(M *m, G *g) {")
        .expect("switch_to_g must exist in zaprt.c");
    let body = strip_c_comments(
        &src[start..start + src[start..].find("\n}\n").expect("switch_to_g must end")],
    );
    for forbidden in ["set_bottom(", "GC_set_stackbottom("] {
        assert!(
            !body.contains(forbidden),
            "I#885: switch_to_g must not call `{forbidden}`: moving Boehm's stack bottom \
             around the context switch leaves a window where a stop-the-world sees sp on \
             one stack and the bottom on the other. Keep the bottom fixed at the native \
             stack top and fix up sp in the GC_set_sp_corrector hook instead."
        );
    }
}

/// The companion half of I#885: with the per-switch bottom updates gone, the
/// scheduler must install the `GC_set_sp_corrector` hook (Boehm builds) that
/// maps a thread captured while running on a coroutine's stack to an empty
/// native range; the coroutine's stack is still scanned as heap through its `G`.
#[test]
fn scheduler_installs_the_boehm_sp_corrector() {
    let code = strip_c_comments(RUNTIME_C);
    assert!(
        code.contains("GC_set_sp_corrector(zap_sp_corrector)"),
        "I#885: the scheduler must install `GC_set_sp_corrector(zap_sp_corrector)` \
         (non-Immix builds) so a thread stopped while running on a green stack is not \
         scanned as a cross-stack range"
    );
}

/// The companion half of the I#869 contract: `switch_to_g` is the ONLY way a
/// coroutine is entered or re-entered, so it must record the resuming M in the
/// G before it swaps the coroutine in (otherwise `g_bootstrap` would read a
/// stale or NULL `run_m`).
#[test]
fn switch_to_g_records_the_resuming_m_before_swapping_in() {
    let src = RUNTIME_C;
    let start = src
        .find("static void switch_to_g(M *m, G *g) {")
        .expect("switch_to_g must exist in zaprt.c");
    let body = &src[start..start + src[start..].find("\n}\n").expect("switch_to_g must end")];
    let set = body
        .find("g->run_m = m;")
        .expect("I#869: switch_to_g must record `g->run_m = m` for g_bootstrap");
    let swap = body
        .find("zap_ctx_swap(&m->sched_ctx, &g->ctx)")
        .expect("switch_to_g must swap the coroutine in");
    assert!(
        set < swap,
        "I#869: `g->run_m = m` must happen BEFORE the coroutine is swapped in"
    );
}
