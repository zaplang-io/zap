# Proposal: `zap playground` — a web playground over the shared `Session` engine

- **Author(s):** Playground lane (I#238)
- **Last updated:** 2026-10-25
- **Status:** Draft — design-first; stops for review before the run/sandbox path is built
- **Discussion:** I#238 (tooling epic I#157, Area E); engine I#159 (REPL), sibling I#234 (doctests)

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

I#238 adds `zap playground`: a local web UI (code editor + **Run**) that
compiles and runs a Zap snippet and shows its stdout, the auto-printed value,
and `ZAPnnnn` diagnostics.
Because the WASM target is post-1.0 (B-track #200–202 deferred), the playground
**cannot run Zap in the browser**; execution is **server-side native** through
the REPL's in-process MCJIT `Session` engine (I#159), so the playground gives
the *same semantics* as `zap run`/`zap repl`/doctests — one executor, no drift.
That makes the server a thing that **runs arbitrary, attacker-controlled native
code**, so the **sandbox is the heart of this ticket**, not the web plumbing.
This note recommends: a tiny `std::net` HTTP/1.1 server (no web-framework dep,
matching how LSP/MCP took no transport dep), a single-shot `POST /run`
(streaming deferred), stateless `#code=` share links (no server storage), and —
the crux — **one short-lived worker *process* per request** under `libc`
resource limits with a parent-enforced wall-clock timeout, output cap, and
concurrency cap, with OS-level sandboxing (seccomp/namespaces/container) called
out as **deployment hardening** layered on top before any public deploy.
It is **design-first**: this note stops for review before the run/sandbox path
is built.

## Background

### What exists (verified against the tree)

- The REPL (I#159, PR #251, merged) factored a reusable **`Session` engine** in
  `crates/driver/src/repl.rs`, behind the `codegen` feature (like `zap run`):
  ```rust
  let mut s = Session::new()?;          // inits the JIT runtime once
  let out: EvalOutcome = s.eval(src);   // compile + JIT-run one snippet
  s.reset();                            // drop accumulated bindings/decls
  ```
- **The landed `EvalOutcome` is narrower than the 0025 sketch.** It is only:
  ```rust
  pub struct EvalOutcome {
      pub diagnostics: Vec<String>,     // "error[ZAPnnnn]: ..." (compile-time)
      pub runtime_error: Option<String>,// uncaught Zap exception, contained
  }
  ```
  There is **no `stdout` and no `value` field**.
  Auto-print and `print(...)` are emitted as real writes to the JIT process's
  **fd 1**; I#234 doctests capture them by redirecting fd 1 (`dup2` → temp file,
  `libc::fflush(NULL)`, restore) around each `eval`, pulling in `libc` as a
  codegen-gated driver dep.
  The playground needs the printed text, so it faces the same choice (see
  §Capturing stdout).
- **Execution is in-process MCJIT with process-global state.** Per I#159: the C
  runtime is compiled and `dlopen`ed **once per process**, with one MCJIT engine
  per `Session`; GC (Immix or Boehm) and the M:N scheduler are **process-global**.
  Two `Session`s in one process share that runtime.
- **Hard faults are not catchable.** An uncaught Zap *exception* is contained
  (`zap_repl_call` returns instead of `exit`), but a **segfault / `abort` /
  stack overflow** in JIT'd native code ends the whole process — documented in
  0025 as an inherent native-REPL limit (Swift/cling included).
- **House style for transports/deps is minimal.** LSP (#229) and MCP (#72) ship
  **stdio only** and reject `--socket` with a clear message — no HTTP/async
  dependency was taken. There is an **in-house JSON** codec in
  `crates/driver/src/json.rs` (`Json::{Obj,Arr,Str,Num,Bool,Null}` with encode +
  parse). `fmt`/`doc` were built in-house. New crates are added sparingly and
  flagged for review.
- CLI dispatch is a hand-written verb table in `crates/driver/src/cli.rs`
  (`run`/`build`/`fmt`/`doc`/`test`/`repl`/`lsp`/…); codegen-only verbs are
  `#[cfg(feature = "codegen")]`.

### Why this is the hard part

WASM would have run untrusted code in the browser's own sandbox, on the user's
machine — the classic playground safety story.
That route is closed until post-1.0.
Server-side native execution moves the untrusted code **onto our host**, so
every safety property the browser sandbox would have given us for free now has
to be built: isolation between requests, containment of crashes, and hard
limits on time, memory, output, filesystem, network, and subprocess creation.

## Proposal

### Architecture

```
browser (editor + Run)
   │  POST /run        { "code": "<snippet>" }
   ▼
zap playground  (parent HTTP server, 127.0.0.1)
   │  spawn one worker process per request, pass snippet on stdin
   ▼
zap playground-worker  (hidden verb; codegen-gated)
   │  Session::new(); capture fd 1; eval(code); Session dropped on exit
   ▼  JSON on stdout:  { stdout, value, diagnostics[], runtime_error, status }
parent: enforce wall-clock + output cap + rlimits, collect JSON, kill on timeout
   ▲
   │  { "stdout": "...", "diagnostics": [...], "runtime_error": null,
   │    "status": "ok" | "error" | "timeout" | "crashed" | "truncated" }
browser renders stdout / value / ZAPnnnn
```

- **Server:** a small blocking HTTP/1.1 server on `std::net::TcpListener`,
  thread-per-connection, bound to **`127.0.0.1` by default**.
  It serves exactly four things: `GET /` (the editor page), `GET /static/*`
  (one CSS + one JS asset, embedded with `include_str!`), `POST /run`, and a
  `GET /healthz`.
  Request/response bodies use the in-house `json.rs`.
  **No web framework, no async runtime** — consistent with LSP/MCP taking no
  transport dep, and the request surface is tiny and fixed.
  Binding to a non-loopback address requires an explicit `--host` **and** prints
  a prominent warning, because that exposes untrusted-code execution to the
  network; the default never does.
- **Editor page:** a single self-contained HTML page — a `<textarea>` (with a
  small vanilla-JS enhancement for tab/indent and Ctrl-Enter to run) plus a
  results pane that renders stdout, the auto-printed value, and each
  `error[ZAPnnnn]` with a link to `zap explain`.
  **No npm, no bundler, no heavy JS framework** (house style). A richer editor
  (CodeMirror-lite) is an optional, deferred enhancement; the first cut is the
  textarea.
- **Reuse:** `/run` drives the **same `Session`/`EvalOutcome`** as the REPL and
  doctests — one semantics. The whole `playground` verb is **codegen-gated**
  like `run`/`repl`; a no-LLVM toolchain build simply has no playground, exactly
  as it has no `zap run`.
- **CLI:** `zap playground [--port N] [--host 127.0.0.1] [--open]`.
  A hidden internal verb `zap playground-worker` is the per-request child (it
  reads a snippet on stdin, runs one `Session`, writes the result JSON on
  stdout). Spawning a child = re-exec of the current `zap` binary with that
  verb, so the worker shares the exact backend build (collector included).

### Single-shot vs streaming — recommend single-shot

**Recommend a single `POST /run` → one JSON result for the first cut**, not
WebSocket/SSE streaming.
Rationale: today the engine's output is captured by an fd-1 redirect **around
the whole `eval`** (I#234) — it is not a live stream, and the output is capped
anyway (see sandbox).
Snippets are short; a single result is the simplest correct thing and needs no
WebSocket dependency.
Because each request already runs in its own worker whose stdout the parent
reads from a pipe, **incremental streaming is a clean, dependency-free
follow-up** (parent forwards the worker's pipe over SSE) if long-running or
chatty snippets ever justify it — recorded as deferred, not designed now.

### Capturing stdout

The playground needs the snippet's printed text, which `EvalOutcome` does not
carry.
The worker process captures its **own fd 1** around `eval` using the same
proven `dup2`→temp-file technique I#234 already uses (and `libc` is already the
codegen-gated driver dep), then puts the captured text into the `stdout` field
of the JSON it emits (the worker writes *its* result JSON to the real fd 1 only
after restoring it).
**Recommended refinement (flag for review):** add a real `stdout: String`
(and later `value: Option<String>`) field to the shared `EvalOutcome` so the
capture lives in the engine once, which would also let the `zap repl` loop and
doctests stop re-implementing fd capture.
I#234's hand-over explicitly suggested I#238 be the ticket that does this.
The first cut can ship with worker-local capture if touching the fresh #159
contract is undesirable; the engine-field refinement is the cleaner end state.

### The sandbox (the crux)

#### Threat model

- **Adversary:** anyone who can reach `POST /run` submits arbitrary Zap, which
  becomes arbitrary **native machine code** with no host-level safety: it can
  loop forever, allocate without bound, `print` without bound, call stdlib
  `os`/`fs`/`net`/`process`, recurse to stack overflow, or trigger a
  segfault/`abort` in JIT'd or runtime code.
- **Assets to protect:** the host process and server availability; other
  requests' state and heap; the host filesystem and secrets; the network
  (no SSRF / outbound abuse / no using us as a relay); host CPU/memory.
- **Security goals:**
  1. The server **survives any snippet**, including one that segfaults/aborts.
  2. **No cross-request leakage** — one request cannot read or corrupt another's
     heap, bindings, or scheduler state.
  3. **Hard bounds** on wall-clock, CPU, memory, and output size.
  4. **No filesystem writes outside a throwaway scratch**, **no network**, **no
     spawning processes** (fork bomb / exec).
  5. No exfiltration of host data.
- **Decisive constraint:** the `Session` engine is **in-process MCJIT with
  process-global GC/scheduler**, and hard faults kill the process.
  Therefore a **shared, long-lived in-process `Session` cannot be the target for
  untrusted input**: one segfault takes the server down, and sessions would
  share a heap (violating goals 1 and 2).
  **Each `/run` must execute in its own short-lived process.**

#### Options

- **(i) One worker *process* per request + `libc` resource limits
  (RECOMMENDED).**
  The parent spawns a child (`zap playground-worker`) that builds **one**
  `Session`, evals the snippet, and exits; the child is the unit of isolation.
  - *Isolation (goal 2):* separate process ⇒ its heap, MCJIT engine, GC, and
    scheduler are entirely its own. Two concurrent requests are two processes
    with nothing shared — this is also exactly what makes the "two concurrent
    requests don't share state" test pass by construction.
  - *Hard-fault containment (goal 1):* a segfault/`abort`/stack-overflow kills
    only the child. The parent sees the non-zero exit / terminating signal and
    reports `status:"crashed"` (with the signal); the **server stays up**. This
    is the single strongest reason for the subprocess model and directly answers
    the ticket's "contained to the worker and reported, not crashing the server."
  - *Wall-clock timeout (goal 3):* the **parent** enforces it — wait on the
    child with a deadline and `SIGKILL` the child's **process group** on expiry
    (killing the group also reaps any coroutines/threads it spawned) → report
    `status:"timeout"`. Portable across macOS and Linux.
  - *CPU timeout:* the child sets `setrlimit(RLIMIT_CPU)` on itself before
    `eval` — a belt-and-braces backstop to the wall-clock (catches a busy loop
    that pins a core even if the parent's timer is delayed).
  - *Memory cap (goal 3):* child sets `RLIMIT_AS` (address space), e.g.
    512 MB–1 GB — generous because MCJIT + Immix/Boehm mmap need headroom.
    *Flag:* `RLIMIT_AS` can interact badly with JIT/allocator mmap and is weaker
    on macOS; validate the cap during impl, and treat a cgroup memory limit as
    the real control in deployment (follow-up).
  - *Output cap (goal 3):* the **parent** reads the child's stdout pipe up to N
    bytes (e.g. 64 KB) and then stops reading and kills the child →
    `status:"truncated"`, returning the prefix. This bounds a runaway `print`
    loop's memory/bandwidth *before* the wall-clock fires. `RLIMIT_FSIZE`
    backstops if output is staged through a file.
  - *No core dumps:* `RLIMIT_CORE = 0`.
  - *Subprocess/exec limits:* `RLIMIT_NPROC` low and `RLIMIT_NOFILE` low in the
    child blunt fork bombs and fd exhaustion (Linux); scratch `cwd` is an empty
    temp dir.
  - *Concurrency/rate cap:* the parent caps in-flight workers with a bounded
    semaphore (e.g. 4); excess requests get `429`/short queue. Per-IP token
    buckets are a deployment follow-up.
  - **No new crate:** everything here is `fork`/`exec`/`setrlimit`/`waitpid`/
    `kill` via `libc`, already a codegen-gated driver dep. On Rust, set the
    rlimits in the child via `std::os::unix::process::CommandExt::pre_exec`.
- **(ii) OS-level sandbox — seccomp-bpf / Linux namespaces / container / macOS
  `sandbox_init` (RECOMMENDED AS DEPLOYMENT HARDENING, layered on (i)).**
  This is what **actually denies** filesystem reads, network, and dangerous
  syscalls — rlimits do not.
  The first cut does **not** ship it because it is platform-specific (Linux
  seccomp vs macOS sandbox vs a container runtime), the deliverable is a
  **local dev server, not a deployed one**, and the concrete mechanism depends
  on the still-pending deployment target (post org-move / public launch).
  The production shape is explicit: run each worker inside a container (or under
  a seccomp syscall allowlist) with **no network namespace**, a **read-only
  rootfs**, a **tmpfs scratch**, and a **cgroup CPU/memory limit** — layered on
  top of the (i) process model, which stays.
- **(iii) A dedicated sandbox-runner crate (landlock/seccomp wrapper, or
  gVisor/Firecracker) (REJECTED for first cut).**
  Heavy dependencies and/or platform lock-in, against house style; the (i)
  primitives via `libc` cover the first cut, and real OS sandboxing in
  production is an orchestration/container concern, not a Rust crate we vendor
  now.

#### Recommendation

**(i) worker-process-per-request** with `libc` rlimits, a parent-enforced
wall-clock timeout, an output cap, and a concurrency cap — **no new crate** —
for the first cut, with **(ii) OS sandboxing as the named production-hardening
follow-up** that must land before any non-loopback/public deployment.

#### First cut enforces vs defers

- **Enforces:** process isolation per request; hard-fault containment with the
  server surviving; wall-clock timeout; `RLIMIT_CPU`; `RLIMIT_AS`; output byte
  cap; `RLIMIT_CORE=0`; low `RLIMIT_NPROC`/`RLIMIT_NOFILE`; empty scratch `cwd`;
  in-flight concurrency cap; loopback-only bind by default.
- **Defers:** true filesystem/network **syscall denial** (OS sandbox, option
  (ii)); per-IP rate limiting; CPU fairness/accounting; streaming output; auth;
  any server-side snippet persistence.

#### Residual risk (and why it is acceptable here)

On a first-cut **dev host**, the worker can still *read* the filesystem and
*reach the network* (rlimits bound resources, not capabilities), and `RLIMIT_AS`
is softer on macOS.
This is acceptable **only because the deliverable is a not-yet-deployed local
server bound to loopback**, run by the developer on their own machine against
their own code.
The OS-sandbox hardening (ii) is called out as a **gating follow-up before the
playground is ever exposed beyond loopback or deployed publicly**, and the
default bind + the warning on `--host` keep the unsafe configuration from
happening by accident.

### Share-a-snippet

Stateless and privacy-preserving: the snippet is encoded into the **URL
fragment** (`#code=<base64url>`), decoded client-side to prefill the editor.
No server storage, no database, no PII, nothing to leak or moderate.
A server-side short-link/paste store is a deferred follow-up that would need
storage plus its own abuse controls, so it stays out of the first cut.

### Dependencies

- **HTTP server:** **none recommended** — `std::net::TcpListener` + a tiny
  hand-written HTTP/1.1 handler + the in-house `json.rs`, consistent with LSP/MCP
  taking no transport dep.
  *Flagged alternative for review:* if hand-rolling HTTP is unwanted, `tiny_http`
  (MIT, no async runtime, one small well-maintained crate) is the minimal vetted
  fallback. `axum`/`actix`/`hyper`+`tokio` are **rejected** (async runtime + a
  large transitive tree, against house style).
- **Sandbox:** **none** — `libc` (already a codegen-gated driver dep) for
  `setrlimit`/`fork`/`exec`/`waitpid`/`kill`, via std's `pre_exec`.
- **Frontend:** **none** — one embedded HTML page + one CSS + one JS asset
  (`include_str!`), vanilla JS only. An optional lightweight editor is deferred.
- **JSON:** **none** — reuse `crates/driver/src/json.rs`.

Net: the recommended first cut adds **zero new crates**.

## Rationale

- **One semantics** is the governing project constraint: the playground drives
  the same `Session`/`EvalOutcome` as `zap run`/`repl`/doctests, so a snippet in
  the playground behaves exactly as it will when shipped.
- The **subprocess-per-request** model is forced by three landed facts —
  in-process MCJIT, process-global GC/scheduler, and uncatchable hard faults —
  and it is also the cleanest way to get isolation and crash containment, and to
  make the required concurrency/isolation tests pass by construction.
- **rlimits + parent wall-clock** give real, portable resource bounds with no
  new dependency; **OS sandboxing** is deliberately deferred to deployment
  because it is platform-specific and the target is undecided, and the honest
  residual risk is bounded by loopback-only + a not-yet-deployed scope.
- **No web framework** mirrors the LSP/MCP decision and keeps the dependency
  surface tiny for a fixed, four-route server.
- Decision to record if accepted: a new **D-0075** — playground over the shared
  `Session` engine; subprocess-per-request sandbox with rlimits + wall-clock +
  output cap; loopback default; OS-sandbox hardening as a deployment follow-up;
  zero new crates; codegen-gated.

## Compatibility

- Purely additive: a new `zap playground` verb (+ hidden `playground-worker`),
  behind the `codegen` feature. The compile path and all existing goldens are
  **unchanged**.
- No IR or runtime-ABI change in the recommended first cut. (The optional
  `EvalOutcome.stdout` refinement, if taken, is an additive engine-API change
  shared with the REPL/doctests, not an ABI change.)
- Any new `.zap` example fixtures must pass `zap fmt --check` (fmt-gate).

## Implementation (phased, after sign-off)

1. **Server skeleton (host-safe parts):** `std::net` HTTP/1.1, the four routes,
   the embedded editor page, `#code=` decode, `json.rs` wiring, loopback bind +
   `--host` warning. `/run` returns a stub until phase 2.
2. **Worker + sandbox (codegen-gated):** `zap playground-worker` running one
   `Session` with fd-1 capture; parent spawn with `pre_exec` rlimits
   (`CPU`/`AS`/`CORE`/`NPROC`/`NOFILE`), scratch `cwd`, wall-clock kill of the
   process group, output-cap reader, concurrency semaphore; map child
   exit/signal → `status` (`ok`/`error`/`timeout`/`crashed`/`truncated`).
3. **`zap playground` CLI + docs:** verb, flags, `--open`; README/SPEC/`zap
   --help`; DECISIONS D-0075; this note → Accepted.
4. **Tests (drive the server in-process):** clean snippet (assert stdout/value);
   `ZAPnnnn` type error (diagnostic surfaces, no crash); runtime `raise`
   (contained + reported); timeout and runaway-output (cap/timeout fires, server
   still answers a follow-up request); **two concurrent requests prove no shared
   state**. Any `.zap` fixtures pass `zap fmt --check`; compile path + goldens
   unchanged.

Gates for the implementation PR: `cargo fmt`; clippy both configs
(`--all-targets -D warnings`); full `make docker-test` under **both** collectors;
in-container clippy both configs; `zap fmt --check tests/golden`; confirm **both**
CI triggers (pull_request AND push) green at run level before reporting. PR
'Closes #238'.

## Open issues

- **Add `stdout`/`value` to `EvalOutcome`?** (engine-shared capture vs
  worker-local fd capture) — recommended, flagged for review.
- **HTTP: hand-rolled `std::net` vs `tiny_http`** — recommend std; confirm the
  no-new-dep stance or approve `tiny_http`.
- **Deployment-hardening mechanism** (seccomp vs namespaces vs container vs
  macOS `sandbox_init`) — deferred until the deployment target is chosen
  (post org-move/public launch); the first cut must not be exposed beyond
  loopback until it lands.
- **Streaming output** (SSE over the worker pipe) — deferred; dependency-free
  when wanted.
- **Server-side share links / paste store** — deferred; `#code=` covers sharing
  without storage for now.
