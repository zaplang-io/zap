# Proposal: 1.0 spec-freeze audit — the ratified Stable set

- **Author(s):** Zap toolchain team
- **Last updated:** 2026-10-27
- **Status:** Draft (STEP 1 — audit + proposal; awaiting maintainer review)
- **Discussion:** issue #206 (epic #158, Area D1). Ratifies the Stable set that
  `docs/STABILITY.md` §4.4 and D-0070/D-0071 explicitly defer to this audit.

> Formatting convention (from Go): wrap near the 80-column mark and start each
> sentence on a new line.

## Abstract

`docs/STABILITY.md` (D-0070) defines what "stable" *means* — the single-version
SemVer scheme, the per-surface bump rules, the three tiers (Stable /
Experimental / Internal), and the Go-1 compatibility promise.
It deliberately defers the one thing it cannot define in the abstract: **which
concrete surfaces are Stable at 1.0**.
That ratification is #206, and this document is it.

Promising stability is a one-way door.
A surface marked Stable at 1.0 is frozen for the entire 1.x series (§3 of the
policy); un-promising it is a MAJOR break.
So the audit is deliberately **conservative**: when a surface is young,
under-specified, platform-fragile, or still has an open question, it ships
**Experimental** and graduates later at a MINOR (policy §4.2) once it has proven
out — which is cheap and additive, whereas the reverse is not.

This proposal is STEP 1: the surface inventory + tiering, the platform matrix
wording, the pre-freeze change list, and the freeze mechanism.
It makes **no** edits to `docs/STABILITY.md` / `SPEC.md` / `docs/GRAMMAR.ebnf`
yet — those land in STEP 2 after the tiering below is approved.
The load-bearing classification calls (the ones that are genuinely a judgement
about whether to promise) are collected in §7 for explicit sign-off, because
ratifying the 1.0 Stable set is the maintainer's decision, not the audit's.

## 1. Method

The audit surveyed the actual tree, not just the docs:

- the language surface against `SPEC.md` §1–§16 + Appendix B and
  `docs/GRAMMAR.ebnf`;
- the stdlib against `crates/sema/src/modules/` (the authoritative module
  registry — 27 compiler-known modules) and D-0056/D-0057/D-0058..D-0063;
- the CLI against `crates/driver/src/cli.rs` (the `Command` enum);
- the package format against `crates/pkg/` (manifest, lockfile, resolver,
  registry) + D-0052 and D-0027;
- diagnostics against `crates/diag/src/lib.rs` (119 registered `ZAPnnnn` codes)
  + D-0071;
- the formatter against `docs/FORMAT.md` + D-0037.

Where the docs and the tree disagree, the disagreement is a **pre-freeze change
list** item (§6), not a silent reclassification.

## 2. Tiering — language (SPEC §1–§16)

Legend: **S** = Stable (frozen for 1.x), **E** = Experimental (shipped, not
promised), **I** = Internal (no contract). "candidate" = recommended Stable but
listed in §7 for explicit confirmation.

| Surface | SPEC / decision | Tier | Note |
|---|---|---|---|
| Lexical structure, literals, line rules | §1, D-0041 | **S** | Locked. |
| Declarations (type-first + `auto`), `const`, mutable-by-default | §3, D-0003/0006 | **S** | Locked. |
| Scalars (`int*`/`f*`/`bool`/`char`/`bigint`/`complex`), wrapping overflow, conversions | §4.1, D-0005/0019/0020/0023 | **S** | Locked. |
| `str`/`bytes`, interpolation, codepoint slicing/views | §4.2, §9, D-0025 | **S** | Locked. |
| `list[T]` / `dict[K,V]` / tuples + value/reference semantics | §4.3, §4.8, D-0023/0049 | **S** | Locked. |
| Structs + methods, `value struct`, composition (no inheritance) | §4.4, D-0015/0050/0051 | **S** | Full OO deferred (out of 1.0 by design). |
| Interfaces (structural, compile-checked) | §4.5, D-0016 | **S** | Locked. |
| Enums tier-1 (named-constant) | §4.6, D-0018 | **S** | Locked. |
| Enums tier-3 (data-carrying sum types) + `match` exhaustiveness | §4.6, D-0046 | **S** candidate | Shipped; Appendix B still says "syntax draft". |
| Operators, true/floor division, comparison, bitwise, boolean, truthiness | §5, D-0022/0024 | **S** | Locked. |
| Operator overloading (dunders, structural op-interfaces), `@derive(Eq,Ord,Hash,Repr)` | §5, D-0047 | **S** | `__iadd__`-family in-place ops **reserved, impl deferred** → that sub-surface is **not-yet** (keep out of Stable). |
| Control flow: `if`/`while`/`for`, `do`/`while`, `match`/`case` | §6, D-0039/0040 | **S** | Locked. |
| Iteration protocol (`__iter__`/comma-ok `__next__`) | §6, D-0048 | **S** | Locked. |
| Functions, lambdas, generators/`yield` | §7, D-0026 | **S** | Locked. |
| Default args, keyword calls, variadics | §7, D-0038 | **S** | Locked. `**kwargs` deferred (not present). |
| First-class function values (`fn(...)->...`) + closures | D-0067 | **S** candidate | Shipped recently (PR#181). |
| Comprehensions & slicing | §8, D-0026/0030 | **S** | Locked. |
| Exceptions (`try`/`except`/`finally`, `raise`, hierarchy) | §10, D-0007/0029 | **S** | Locked. |
| Context managers / `with` | §11, D-0027/0069 | **S** | Locked. |
| Modules, imports (incl. grouped/multi/alias), `_`-visibility | §13, D-0028/0052 | **S** | Locked. |
| Regex (PCRE2, `m`/`s`, `=~`/`!~`/`~=`, named captures) | §14, D-0010 | **S** | Locked. |
| Decorators & annotations core | §16, D-0036 | **S** | The `@`-mechanism + annotation/decorator forms. |
| Generics incl. bounds (user-defined, monomorphized) | §4.7, D-0017/0064 | **S** candidate | Shipped recently (PR#156/166); first vertical. |
| Concurrency: coroutines, `spawn`, channels, `close`, comma-ok recv | §12, D-0008/0031 | **S** | Model locked. |
| `select`, structured `scope:`, `spawn detached`, `OnError`, `ExceptionGroup` | §12, D-0044/0045 | **S** candidate | Appendix B marks the *syntax* "draft"; confirm freeze. |
| Sync primitives (`Mutex`/`RWMutex`/`WaitGroup`/`Once`) + `Atomic[T]` | §12, D-0032/0066 | **S** candidate | `Atomic[T]` shipped recently (PR#177). |
| `prelude Optional[T]` | D-0065 | **S** | Canonical optional. |
| **Numeric arrays / SIMD, dtype promotion** | §15, D-0011 | **E** | Promotion rules open (OPEN-QUESTIONS); incremental. |
| **`set` type** + set comprehensions | §4.3, D-0043 | **E** | Appendix B lists `set` as *Open*; type exists, surface not settled. |
| **C-style bitfields on `value struct`** | D-0068 | **E** candidate | Niche, shipped recently (PR#182); low-churn-risk but young. |
| **Python interop (`import python`)** | §13, D-0003/design 0003 | **E** | "Rare relief valve"; depends on host CPython. |
| **`@derive(Json)` / `@derive(Yaml)` / `@derive(Csv)` + `to_*`/`from_*`** | D-0059/0063 | mixed | `@derive(Json)` **S** candidate (core serde); **Yaml/Csv E** (subset/young). |
| Value/memory layout, enum/closure representation, GC algorithm | §1.5 | **I** | Never promised (whole-program AOT). |

## 3. Tiering — standard library (per module)

The tree ships **27** compiler-known modules (`crates/sema/src/modules/`), plus
import-free built-ins.
`strings`, `collections`, `csv`, `testing` from `SPEC.md` §18 are **not**
importable modules (see §6) — they are built-in `str` methods, built-in
containers + the `deque`/`bisect`/`oset` modules, a `@derive(Csv)`, and the
`@test` framework respectively.

| Module / built-in | Decision | Tier | Note |
|---|---|---|---|
| Built-ins: `print`/`len`/`range`/`assert`, scalar/container ctors, `heap[T]` | D-0056/0057 | **S** | Core; `heap` locked. |
| `io`, `fs` | D-0056 (#97) | **S** candidate | Core; audit the exact fn list before freeze. |
| `os`, `sys`, `process` | D-0056 (#97) | **S** candidate | Core; `process` surface is small — confirm. |
| `math`, `random` | D-0056 (#98) | **S** candidate | Core. `random` default-RNG determinism = unspecified (not promised). |
| `time`, `datetime` | D-0056 (#99) | **S** candidate | Core. |
| `collections` extras: `deque`, `bisect`, `oset` | D-0056 | **S** candidate | Confirm these freeze vs stay E. |
| `sync` | D-0032 (#103) | **S** | Pairs with the concurrency model. |
| `json` | D-0059 (#100) | **S** candidate | Core serde; three modes + `@derive(Json)`. |
| `logging`, `argparse` | D-0056 (#101) | **S** candidate | Common app surface. |
| `crypto` | D-0056 (#105) | **E** candidate | Security-sensitive surface; recommend **E** until the primitive list + guarantees are audited (promising a crypto API is high-stakes). |
| `net`, `http` | D-0061 (#104) | **E** | Plaintext TCP + HTTP/1.1 client; no HTTP/2, minimal server — too young to freeze. |
| `tls` | D-0062 (#147) | **E** | Links system OpenSSL; `https` client only; platform-fragile. |
| `zlib`, `gzip` | D-0060 (#106) | **E** candidate | Thin system-zlib wrappers; small — could be **S**, recommend **E** to pair with net/http. |
| `yaml` | D-0063 (#149) | **E** | Subset YAML; young. |
| `threading` (compat) | I#175 | **E** | Migration-only; prefer coroutines (per seed). |
| `signal` | D-0058 (#77) | **S** (API) / **I** (dump format) | `signal.notify`/`@on_signal`/`scope(cancel_on=…)` API Stable; crash-dump text Internal. |
| `gc`, `runtime` | D-0058 | **S** (API) / **I** (output) | `gc.collect()`/`gc.stats()`/`runtime.coroutine_dump()` names Stable; dump/stats *format* Internal (§3.1). |

## 4. Tiering — CLI, package format, diagnostics, fmt

### 4.1 CLI verbs (`crates/driver/src/cli.rs`, SPEC §17)

| Verb | Tier | Note |
|---|---|---|
| `run`, `build`, `test`, `fmt`, `new`, `init`, `add`, `remove`, `version`, `help` | **S** | Core verbs + their flags/exit codes. |
| `zenv` (`create`/`status`/`path`/`clean`) | **S** candidate | D-0052 project env. |
| `explain <ZAPnnnn>` | **S** | The *command*; its prose output is not promised (§3.1). |
| `doc` (`--format html\|md\|json`) | **S** (verb) / **E** (`--json` model, html layout) | **Missing from SPEC §17** (see §6). |
| `test --json`, `mcp`, `doc --format json` **schemas** | **E** | Per §3.1 a `--json` schema is covered only once explicitly stabilized; none is yet. |
| `mcp`, `lsp`, `repl`, `playground` | **S** (verb exists) / **E** (protocol/semantics) | Agent/editor/interactive surfaces; `lsp` **missing from SPEC §17**. |
| `ir`, `expand`, `inspect` | **S** (verb) / **I** (output) | Diagnostic/introspection output Internal (§3.1). |
| `playground-worker` | **I** | Hidden internal subprocess; "not for direct use". |

### 4.2 Package format

| Surface | Decision | Tier | Note |
|---|---|---|---|
| `zap.toml` manifest, `zap.lock`, MVS resolver, `git`/`path` sources, checksum DB | D-0052 | **S** | Per policy §1.4 additive rule. |
| Registry index + `registry` source + `Source::Registry` (**read path**) | D-0027 | **E** | New (PR#257); **read-only** — no publish path shipped. Freeze the manifest *keys* as additive-ignorable, keep registry *behavior* Experimental. |

### 4.3 Diagnostics (`ZAPnnnn`, D-0071)

119 codes registered: meta 2, lex 8, parse 14, name 3, type 32, semantic 53,
back-end 7 (no reserved-band codes in use — clean).

| Surface | Tier | Note |
|---|---|---|
| The `ZAPnnnn` scheme + category grouping + `zap explain` | **S** | Per §3.4. |
| The **meaning** of each non-meta code (lex/parse/name/type/semantic/back-end) | **S** candidate | Ratify as the Stable code set; see §7. |
| `ZAP0001` (ICE), `ZAP0002` (feature-not-yet-implemented) — the meta band | **E/I** | Recommend **not** Stable: NYI is by definition transient; ICE is Internal. |
| Message **text** of every code | not stable (ever) | §3.4 — key off the code. |

### 4.4 `zap fmt` canonical style (D-0037, FORMAT.md)

| Surface | Tier | Note |
|---|---|---|
| `zap fmt` / `zap fmt --check`, its parse-preserving + idempotence contract, exit codes | **S** | The tool and its guarantees. |
| The **exact byte-for-byte canonical rendering** | **not a stability promise** | Recommend the Go/`gofmt` stance: the formatter output may be refined within 1.x (a reformat is never a semantic change). Freezing exact bytes would make every whitespace tweak a MAJOR break forever. See §7. |

## 5. Platform matrix (STABILITY.md §7.3)

Honest wording: "Stable" for a platform means the §3 promise is **continuously
CI-verified** on it.
A platform whose CI is not currently green cannot honestly be called Stable, no
matter how complete the implementation is.

| Platform | Proposed 1.0 tier | Basis |
|---|---|---|
| **Linux x86-64** | **Stable (tier-1, CI-gated)** | Primary dev/CI target; both CI triggers green. |
| **Windows x86-64** | **Stable (tier-1, CI-gated)** | A-track #195–199 merged; CI-gated. |
| **macOS x86-64** | **Supported — not yet CI-gated** (tier-2) | Implemented (PR#259) but **parked**: the required macOS runner was unavailable/cancelled. Wording must say the impl exists and is expected to work, but is **not** under the §3 CI-verified promise until the runner lands and PR#259 merges — then it graduates to Stable at a MINOR. |
| **macOS arm64 (Apple Silicon)** | **Experimental — known issue** | I#260: JIT exception-unwinding gap. AOT (`run`/`build`) is fine; `repl`/`playground`/doctests (JIT) are affected. |
| **aarch64-Linux** | **Experimental — known issue** | I#223: multicore Immix GC use-after-free (evacuation race), parked. |
| **WASM** | **Post-1.0** | Fast-follow (epic #158); not a 1.0 target. |

Recommendation: state the tier definitions explicitly in §7.3 — **tier-1**
(CI-gated, Stable), **tier-2** (supported/best-effort, CI pending or partial,
**not** under the promise), **experimental** (known issues, linked) — so
"supported" never gets read as "promised".

## 6. Pre-freeze change list

Flagged as **fix-before-freeze** (F), **mark-Experimental** (E), or
**leave-as-is** (L). No sweeping renames are proposed; rename candidates are
listed for review only.

1. **(F) `SPEC.md` §18 lists modules that are not importable modules.**
   `strings`, `collections`, `csv`, `testing` are documented as "bundled std
   imported by flat short name", but the tree has no such modules.
   Reconcile: `strings` = built-in `str` methods; `collections` = built-in
   containers + `deque`/`bisect`/`oset`; `csv` = `@derive(Csv)` + `to_csv`/
   `from_csv`; `testing` = the `@test` annotation framework.
   Doc fix only — no code change.
2. **(F) `SPEC.md` §13 example `import strings`** is not valid (same cause).
   Replace with a real module (`import collections as col` already shown; use an
   implemented module in the `import strings` slot, e.g. `import math`).
3. **(F) `SPEC.md` §17 omits shipped verbs `doc` and `lsp`.** Both exist in
   `cli.rs` (I#232/233, I#229). Add them to the §17 listing.
4. **(F) `docs/GRAMMAR.ebnf` header says "tracks D-0001..D-0058"** but the tree
   is at D-0075. Grammar predates first-class fn types (D-0067), `with`/context
   managers (D-0069), bitfields (D-0068), `Atomic[T]` (D-0066), `Optional[T]`
   (D-0065). Audit the grammar against §2's Stable language set and bump the
   header to D-0075 before stamping it 1.0.
5. **(F) `SPEC.md` Appendix B** still marks structs/enums "syntax draft" and
   concurrency "`select`/structured-concurrency syntax draft". If those freeze
   Stable (§7), Appendix B must be upgraded to a per-section **Tier** column and
   drop the "draft" hedges; if not, they move to Experimental.
6. **(E) `crypto`, `net`, `http`, `tls`, `yaml`, numeric arrays/SIMD, `set`,
   registry, Python interop, `threading`, `@derive(Yaml/Csv)`** — mark
   Experimental rather than Stable (rationale per-row in §2–§4). This is the
   conservative core of the freeze.
7. **(E) meta diagnostic band `ZAP0001`/`ZAP0002`** — exclude from the Stable
   code set (ICE is Internal; NYI is transient).
8. **(L) `coroutine`-not-`goroutine`** — swept clean; no residue in `SPEC.md`
   or `GRAMMAR.ebnf` (the `goroutines-25` feature file is a historical ticket
   name, not surface). Nothing to fix.
9. **(review-only) rename candidates** — none proposed. The `jsonlib`/`boto`
   import examples in §13 are illustrative, not stdlib. No naming inconsistency
   rises to a rename.

## 7. Load-bearing classification calls (for maintainer sign-off)

These are the calls where "Stable vs Experimental" is a genuine judgement, not a
mechanical read. Each is a one-way door at 1.0.

1. **Recently-shipped language features as Stable:** generics+bounds (D-0064),
   first-class fns+closures (D-0067), `Atomic[T]` (D-0066), data-carrying enums
   (D-0046), structured-concurrency syntax (D-0044/45). All work and are
   documented; all are < ~6 weeks old. **Recommend Stable** (they are core and
   well-exercised), but each is a deliberate freeze — confirm.
2. **C-style bitfields (D-0068):** niche value-struct feature. **Recommend
   Experimental** (young, narrow use) unless you want it frozen now.
3. **`crypto` as Experimental:** promising a crypto API (primitive set,
   algorithm guarantees) at 1.0 is high-stakes and hard to walk back.
   **Recommend Experimental** even though it's "core-ish".
4. **`zlib`/`gzip` as Experimental vs Stable:** thin and stable in shape;
   recommended Experimental only to pair with net/http. Could reasonably be
   Stable. Your call.
5. **`collections` extras (`deque`/`bisect`/`oset`) Stable vs Experimental:**
   small, Python-familiar surfaces. Recommend Stable; confirm.
6. **The diagnostic Stable code set = all 117 non-meta codes:** this freezes
   each code's *meaning* within 1.x. Recommend ratifying the whole non-meta set;
   the alternative is to Stable-freeze only a reviewed subset and leave the rest
   Experimental. Your call on blanket vs subset.
7. **`zap fmt` exact output is NOT promised (Go stance):** this is the single
   most consequential formatting call. Recommend **not** freezing exact bytes so
   the canonical style can still be refined within 1.x. Confirm you're happy
   following gofmt here (agents re-run `zap fmt`, so churn is absorbed).
8. **macOS x86-64 wording = "Supported, not yet CI-gated" (tier-2), not
   Stable.** This is honest but means macOS is *not* under the 1.0 promise at
   ship. Confirm, or hold 1.0 for the macOS runner + PR#259.

## 8. The freeze mechanism (how it is recorded)

Proposed for STEP 2 (after approval):

1. **`docs/STABILITY.md`:** replace the provisional §4.4 table with an
   **authoritative "§4.4 The ratified 1.0 Stable set"** carrying the §2–§4
   tiering of this doc, and fill in **§7.3** with the §5 platform matrix + the
   tier definitions and the #223/#260 links.
2. **`SPEC.md`:** add a **Tier** column to Appendix B (per-section
   S/E/I), apply the §6 fixes (F1–F5), add explicit **"Frozen for 1.0 (Stable)"**
   markers to the Stable sections, and stamp the document **1.0** (reconciling
   the editorial `Version:` with the toolchain MAJOR, per policy §2).
3. **`docs/GRAMMAR.ebnf`:** audit against the Stable language set, bump the
   header to D-0075, stamp 1.0.
4. **New `DECISIONS.md` D-0076** recording the freeze: "the #206 audit ratifies
   the Stable set per `design/0029-spec-freeze.md`; this is the authoritative
   1.0 surface classification."
5. **Additivity post-freeze:** new surfaces land Experimental and graduate to
   Stable at a MINOR via policy §4.2 (DECISIONS entry + full docs); the Stable
   set only ever grows additively within 1.x (policy §1.2/§1.4). No per-surface
   machinery beyond the tier annotations is needed for 1.0 — the `[experimental]`
   opt-in gate (STABILITY §4.1) is tooling-track work, not a freeze blocker.

STEP 2 is doc/policy only: no code, no behavior change, no sweeping renames
unless explicitly approved here. All `.zap` fixtures stay fmt-clean and the
compile path + goldens are untouched.
