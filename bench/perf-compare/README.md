# Throwaway perf compare — Zap vs Go vs Python

**Throwaway.** This directory exists to eyeball where the #568 codegen work (frame
skip + inlined safepoint polls, D-0144) lands Zap relative to a C-family AOT
language (Go) and a bytecode interpreter (CPython), on a few small algorithms. It
is **not** a regression gate and is **not** meant to merge — the standing,
machine-independent perf guard lives in
[`crates/driver/tests/perf.rs`](../../crates/driver/tests/perf.rs). Read the
numbers from the `Perf compare (throwaway)` workflow's run summary, then close the
PR.

## Benchmarks

Each program is written identically in all three languages and prints the same
result (the harness checks the output, so a miscompile can't flatter a timing):

| file | what | expected output |
|---|---|---|
| `fib` | `fib(32)`, naive recursion — call-heavy, GC-free | `2178309` |
| `loop_sum` | sum `0..30_000_000` — tight integer loop, GC-free | `449999985000000` |
| `dict_string` | build + look up 200k string→int entries — GC-heavy | `19999900000` |
| `dict_int` | same workload with **int keys** — isolates the dict cost | `19999900000` |
| `fstring_only` | build 400k `f"k{i}"` and sum their `len` — isolates the string-construction cost | `2577780` |

`dict_string ≈ fstring_only + dict_int` splits the string-key workload into its
two halves, so the table shows whether the cost is the dictionary or the
per-key string construction.

## Running

In the LLVM-18 environment (the CI job does this for you):

```bash
cargo build --release -p zapc --bin zapc --features gc-immix
ZAPC="$PWD/target/release/zapc" REPS=5 bench/perf-compare/run.sh
```

Each program is compiled with its language's **default** toolchain (`zapc build`,
`go build`, CPython) and run best-of-`REPS`. Absolute milliseconds are
machine-dependent — the ratios (how many times faster Zap is than Go / Python)
are the comparable figure.
