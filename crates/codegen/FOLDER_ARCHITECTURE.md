# `crates/codegen` runtime C — folder architecture

The single place that documents how the C runtime is split and embedded, and the
checklist for adding a new fragment. Read this before touching `zaprt.c`, the
`runtime/` fragments, or the embedding plumbing.

## What the runtime C is and how it reaches a binary

The Zap C runtime is **one translation unit**: `crates/codegen/src/zaprt.c`. It
`#include`s a set of C **fragments** under `runtime/` textually — there is a single
`cc`/`clang` compile over `zaprt.c`, and the fragments are **not** separate object
files. Cross-fragment calls resolve inside the one TU, so a fragment only ever needs
a forward declaration (or an earlier `#include`); **there are no link-time cycles.**

The whole runtime is embedded into the compiler binary with `include_str!`, listed
once in `runtime_manifest.rs`, and written back out to a temp build dir at
compile/REPL time, then compiled by `cc`.

```
zaprt.c ──#include──▶ runtime/<module>.inc.c … , runtime/accel/*
   │  embedded via include_str!  (RUNTIME_FRAGMENTS manifest = one source of truth)
   ▼
compiler binary ──write_tu_fragments()──▶ build dir ──▶ cc -fexceptions [-flto] -O2 ──▶ runtime object/dylib ──▶ linked into the user's program
```

Separately linked, **not** fragments (conditionally-linked standalone objects —
leave alone): `zap_ctx_win_x64.c`, `zap_win_a3.c`, `zap_np_iocp.{c,h}`, `zapfft.cpp`,
`zapxsort.cpp`, `zaphwysort.cpp`.

## File map

- `zaprt.c` — the master TU: **core runtime** (platform detection, GC abstraction,
  the M:N work-stealing scheduler, coroutines + channels, the A1 context-switch
  seam, async preemption, `lock_os_thread`, the netpoller, signals/diagnostics, heap
  type descriptors) **plus** the `#include` list that pulls in every fragment.
- `runtime/` — one `.inc.c` per stdlib module body, lifted out of `zaprt.c`:
  `array`, `str`, `os_fs`, `crypto`, `bytes`, `time_datetime`, `math_random`,
  `json`, `xml`, `url`, `collections`, `logging_argparse`, `csv`, `yaml`, `zlib`,
  `sync`, `net_http`, `tls`, `crypto_jose`, `threading`, `profile`, plus the core
  fragments `gc.inc.c` and `race.inc.c`.
- `runtime/accel/` — the #139 ISA-acceleration + numeric family: `blas`, `random`,
  `cpu`, `memchr`, `arch`, `sortaccel` (`.inc.c`) and the headers `zap_cpu_features.h`
  and `ziggurat_constants.h` that those fragments `#include` (kept beside them so the
  relative include resolves).
- `runtime_manifest.rs` — **the single source of truth** (`RUNTIME_FRAGMENTS`): one
  `RuntimeFile { path, text, in_tu }` row per fragment/header. `write_tu_fragments()`
  is the one write loop (replaces the old lib.rs + jit.rs write sites);
  `hash_runtime_fragments()` is the one cache-key hash (replaces the two hand-kept
  `RUNTIME_*.hash` blocks).

## The one model you must hold in your head

`#include` **order in `zaprt.c` is semantic**: a fragment may use macros, typedefs,
`static` helpers, or an open `#if` from earlier in `zaprt.c` or an earlier fragment.
(Example: the `ZAP_NO_OPENSSL` guard opens in `runtime/tls.inc.c` and closes in
`runtime/crypto_jose.inc.c` — they are `#include`d consecutively, so the nesting is
preserved.) A fragment is therefore **not** a standalone TU and will not compile
alone. When you move code into a fragment, place its `#include` at the exact point
in `zaprt.c` the code used to occupy, so global order is unchanged.

Because every fragment is `#include`d at its original position, lifting a module
body is **byte-identical**: the preprocessor sees exactly the same token stream.

## GUIDELINES: adding a NEW stdlib module (or runtime fragment)

Two edits — and the drift-guard test (`cargo test -p zapc_codegen`, no LLVM needed)
catches you if you miss one.

1. **Create the fragment** `runtime/<module>.inc.c` (or `runtime/accel/<x>.inc.c`).
   If it needs a private header, put the `.h` beside it so its relative `#include`
   resolves.
2. **Add a row to the manifest** (`runtime_manifest.rs`):
   ```rust
   RuntimeFile { path: "runtime/<module>.inc.c",
                 text: include_str!("runtime/<module>.inc.c"), in_tu: true },
   ```
   That one row drives embedding, both write paths (`write_tu_fragments`), and both
   cache-key hashes (`hash_runtime_fragments`). Use `in_tu: false` only for a file
   that is a separate object yet still affects the runtime (the SIMD sort kernels).
3. **`#include` it in `zaprt.c`** at the correct position (order is semantic). This
   is the one hand-edit that stays in `zaprt.c`.
4. **Tests:** behavioral coverage for a stdlib module lives in the Zap goldens
   (`tests/goldens/...`) / driver tests, not here. Add a Rust source-contract test to
   the codegen crate only if you need to pin a C-source invariant.
5. **`build.rs`:** nothing to do — it globs `src/runtime/**` for `rerun-if-changed`.
6. **Before push:** `cargo fmt --all` + `cargo clippy`, and
   `cargo test -p zapc_codegen` (runs the drift-guard + source-contract tests with
   no LLVM). For a pure move, confirm byte-identity: either reconstruct `zaprt.c` by
   expanding its `runtime/` `#include`s and diff against the pre-split file, or
   `clang -E -P` the master before/after and diff.

## Invariants (enforced by the drift-guard test `runtime_manifest_matches_disk_and_includes`)

- Every file under `src/runtime/` has a `RUNTIME_FRAGMENTS` row, and every `runtime/`
  row exists on disk.
- Every `.inc.c` row is `#include`d by `zaprt.c`.
- Every `#include "runtime/…"` in `zaprt.c` has a manifest row.
- Every embedded fragment text is non-empty.
- (Build-time, #841) the runtime is compiled `-fexceptions` **before** any LTO merge:
  an unwinding runtime built without it drops exception landing pads under `-O2`.

## Why this exists

Before the manifest, a fragment had to be wired in six hand-maintained places
(`include_str!`, the `#include`, the lib.rs write site, the jit.rs write site,
`build.rs`, and the cache-key hash twice). They drifted: `cpu`/`memchr`/`arch`
shipped without being added to either cache-key hash, so editing only one of them
could reuse a stale runtime object (the #678 class of bug). The manifest + drift
test make that class of mistake fail fast, on a laptop.
