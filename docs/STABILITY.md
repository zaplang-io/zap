# Zap — Versioning & Stability Policy (the 1.0 contract)

**Status:** Locked (policy). See `DECISIONS.md` D-0070, issue #205, epic #158
(`design/0016-platform-1.0.md`, Area D2).
**Last updated:** 2026-10-22

This document defines what the word **"stable"** means for Zap: the versioning
scheme, what a version bump promises for each surface, how features move between
stability tiers, and the rules for deprecating and breaking things. It is the
contract the phrase **"public 1.0"** refers to.

It is deliberately concrete and enforceable, not aspirational. Where a rule says
*must* / *only* / *never*, it is a gate on the release process (#207) and on
every pull request that touches a public surface.

> **Scope of the promise.** The compatibility *promises* in this document
> (§3–§6) take effect at the **1.0.0** release. Before 1.0 we are in the 0.x
> regime of §2, which makes **no** compatibility promise. This document can be
> amended by the normal design-doc + `DECISIONS.md` process; amendments that
> weaken a promise already in effect are themselves a breaking change (§6).

---

## 1. The scheme — SemVer 2.0.0 over a single toolchain version

Zap is distributed as **one artifact**, the `zap` toolchain (compiler, runtime,
bundled standard library, CLI, and package tooling). It carries **one version**
— the `[workspace.package] version` in `Cargo.toml` — and that version is
[Semantic Versioning 2.0.0](https://semver.org): `MAJOR.MINOR.PATCH`, with
optional pre-release (`-alpha.N`, `-beta.N`, `-rc.N`) and build metadata.

A single version covers several distinct **surfaces**. The meaning of each
component is defined per surface, and **the release's bump is the maximum over
all surfaces** of the change it contains (a breaking change to any one stable
surface forces a MAJOR; a pure addition anywhere forces at least a MINOR).

| Surface | What it is |
|---|---|
| **Language** | Syntax and semantics of `.zap` source (`SPEC.md`, `docs/GRAMMAR.ebnf`, the `D-00xx` decisions). |
| **Standard library** | The documented API of built-ins and bundled `std` modules (`SPEC.md` §18). |
| **CLI** | The `zap` subcommands, their flags, exit codes, and the schema of any `--json` output (`SPEC.md` §17). |
| **Package format** | `zap.toml`, `zap.lock`, the module-cache and checksum-DB layout, the resolver's observable behavior (D-0052). |
| **Runtime / ABI** | The C runtime (`zaprt`), generated-code layout, the eventual C FFI surface. **See §1.5 — not a public surface in the whole-program model.** |

### 1.1 MAJOR — `X.0.0`

Incremented for a **backward-incompatible change to any *stable* surface**:

- **Language:** a program that compiled and ran under the previous MAJOR no
  longer compiles, or compiles but behaves differently in a way the spec does
  not already license (see the bug-fix carve-out, §3.2).
- **Standard library:** a stable API is removed, or its signature or documented
  contract changes incompatibly.
- **CLI:** a stable subcommand/flag is removed or changes meaning; a stabilized
  `--json` schema changes incompatibly; exit-code meaning changes.
- **Package format:** a change a current tool cannot read or resolve the same
  way (see §1.4 for the additive rule that keeps most format changes MINOR).

Post-1.0, a MAJOR bump is **rare and governed by §6** (breaking-change process).
Removing a platform from the support matrix (§7) is a MAJOR change.

### 1.2 MINOR — `X.Y.0`

Incremented for a **backward-compatible addition**. Every program valid under
`X.(Y-1)` must still compile and behave the same under `X.Y`:

- new syntax that does not change the meaning of any previously valid program;
- new standard-library modules, types, functions, or additional optional
  parameters that do not alter existing call sites;
- new CLI subcommands, new flags, new fields appended to a `--json` schema;
- **graduating an Experimental surface to Stable** (§4);
- **deprecating** a stable surface (the surface keeps working — §5);
- additive package-format changes (§1.4);
- performance improvements, new target platforms (§7), MSRV bumps (§7.4).

### 1.3 PATCH — `X.Y.Z`

Incremented for **bug fixes with no surface change**: no new syntax, API, flag,
or format field, and no *intended* behavior change to a **conforming** program.
Fixing a miscompilation, a crash, or a spec-conformance bug is a PATCH even
though it changes the behavior of the *non-conforming* program that depended on
the bug — see the bug-fix carve-out in §3.2.

### 1.4 The package-format additive rule

`zap.toml` and `zap.lock` are a cross-tool, cross-version contract. Within a
MAJOR:

- New keys/fields are **additive and ignorable**: an older `1.x` tool must not
  error on a key it does not recognize that a newer `1.y` tool wrote (it may
  warn). This keeps format evolution at MINOR.
- A change that makes an older `1.x` tool **misresolve or refuse** a project
  written by a newer `1.y` tool is breaking → MAJOR.
- `zap.lock` is tool-generated; its exact serialization may change at MINOR as
  long as any `1.x` tool can consume any `1.x` lockfile. Lockfile **semantics**
  (what a pinned resolution means) are stable within a MAJOR.

### 1.5 Runtime / ABI is *not* a public surface

Zap compiles **whole-program, ahead-of-time** (D-0021): every Zap unit in a
program is compiled together by one toolchain version. There is **no** dynamic
linking of separately-compiled Zap code and therefore **no stable binary ABI
between Zap units or between compiler versions**. Concretely, at 1.0 and within
every MAJOR, Zap makes **no promise** about:

- the `zaprt` C runtime ABI, struct layouts, calling conventions, or the GC
  type-descriptor format;
- generated-code layout, symbol names, or object-file compatibility between two
  compiler versions;
- the on-disk/in-memory representation of any value (tagging, padding, pointer
  vs inline — e.g. D-0051/D-0067 value structs, D-0046 enum representation).

A binary produced by the compiler is **self-contained** for its target; two
compiler versions are not expected to produce link-compatible artifacts. The
**C FFI** surface (D-0012, deferred) is the one runtime-adjacent surface that
will become public *when it ships* — and it will land as **Experimental** (§4)
and be governed by this policy from then on. Until then, everything under
"Runtime / ABI" is **Internal** (§4) and may change in any release.

---

## 2. The 0.x regime (where we are now)

The current version is `0.0.1`; `SPEC.md` is a "0.2 draft, living document".
Under SemVer, **`0.x` makes no compatibility promise** — any release may change
anything. Zap adopts that fully but imposes a light internal discipline so the
history stays legible:

- Version is `0.MINOR.PATCH`. A **`0.MINOR` bump may contain breaking changes to
  any surface**, including the language. A **`0.x.PATCH` bump is bug-fixes
  only** (same no-surface-change rule as §1.3).
- Every change that would be MINOR-or-higher post-1.0 still gets a `CHANGELOG`
  entry, and every locked language/stdlib decision still gets a `DECISIONS.md`
  entry. We minimize gratuitous churn, but we **do not** promise stability yet
  and **do not** run the deprecation window (§5) during 0.x — a surface can be
  removed in the next `0.MINOR` with only a `CHANGELOG` note.
- **Two version numbers, one authority.** The `SPEC.md` `Version:` header (e.g.
  `0.2`) is an **editorial revision of the spec document** and moves
  independently of the toolchain version in `Cargo.toml`, which is the
  **authoritative release version** for everything in this policy. At the 1.0
  release the two are reconciled: `SPEC.md` is stamped `1.0` and tracks the
  language MAJOR thereafter.
- **Pre-releases** on the road to 1.0 use SemVer pre-release tags:
  `1.0.0-alpha.N` → `1.0.0-beta.N` → `1.0.0-rc.N` → `1.0.0`. A pre-release makes
  no stability promise; it exists to let the stable set (§4, #206) be exercised
  before it is frozen.

---

## 3. The 1.0 promise — what it covers

Once `1.0.0` ships, Zap adopts a **Go-1-style compatibility promise**:

> **A program that compiles and runs correctly under `1.0`, using only Stable
> surfaces, will continue to compile and run with the same behavior under every
> `1.x` release.**

This holds for the **entire life of the `1.x` series** (§7.1) and covers, for
the surfaces classified **Stable** by the 1.0 audit (§4, #206):

1. **Language** — the syntax and semantics in `SPEC.md` marked Stable: a valid
   Stable program keeps compiling and keeps its specified behavior.
2. **Standard library** — the documented Stable API: names, signatures, and
   documented contracts are preserved; only additions (§1.2) are made.
3. **CLI** — Stable subcommands, their flags, exit codes, and any `--json`
   schema explicitly stabilized for agent/tool consumers.
4. **Package format** — `zap.toml` / `zap.lock` per §1.4; a `1.x` toolchain
   resolves and builds any `1.y≤x` project.

### 3.1 What it explicitly does **not** cover

The promise is bounded. None of the following is covered, and relying on it is
not supported — it may change in **any** release, including a PATCH:

- **Experimental and Internal surfaces** (§4). Everything not on the Stable list
  produced by #206 is outside the promise, whether or not it happens to be
  reachable.
- **Bug-for-bug behavior.** We reserve the right to fix bugs; see §3.2.
- **Timing, performance, and resource use** — GC algorithm and pauses (Boehm vs
  precise Immix, D-0009), scheduler fairness and ordering of coroutines,
  allocation behavior, compile times, binary size.
- **Unspecified ordering** — iteration order of `dict`/`set` and any collection
  whose order the spec does not pin; map randomization.
- **Value representation and memory layout** — see §1.5.
- **Diagnostics** — the text, formatting, and wording of error/warning messages,
  stack-trace rendering, and `--help` prose. (A `--json` diagnostic schema is
  covered **only** once explicitly stabilized.) **Exception:** the stable
  `ZAPnnnn` **error codes** — their numbers and documented *meaning*, not the
  message prose — are a covered surface once marked Stable; see **§3.4**.
- **Diagnostic/introspection command output** — `zap ir`, `zap expand`, and the
  format of runtime dumps (`SIGQUIT`/`runtime.coroutine_dump()`, D-0058).
- **The runtime/ABI** (§1.5) and anything under `design/IR.md`,
  `design/RUNTIME-ABI.md`, and the `zaprt` internals.
- **Security fixes** — see §3.3.
- **Platform additions** — gaining support for a new platform is non-breaking
  (§7); only *dropping* one is breaking.

### 3.2 The bug-fix carve-out

A conforming program is one whose behavior is determined by `SPEC.md` and the
Stable surfaces. If a program's behavior depends on an **unspecified** detail,
or on a **defect** (a miscompilation, a crash, a divergence from the spec), then
fixing that defect is **not** a breaking change even though the program's
observable behavior changes. Such fixes ship at PATCH or MINOR. When a fix is
expected to affect real code in practice, it is called out in the `CHANGELOG`.

This is the standard escape valve (as in Go and Rust); without it, no bug in a
1.x release could ever be fixed.

### 3.3 The security / soundness carve-out

A fix for a security vulnerability or a type-/memory-safety unsoundness may, if
there is no compatible alternative, change the behavior of a Stable surface in a
**MINOR or PATCH** release. Such a change:

- is scoped **as narrowly as possible** to the vulnerable behavior;
- is documented prominently in the `CHANGELOG` and `SECURITY.md`, with the
  rationale and any migration;
- requires a `DECISIONS.md` entry and maintainer sign-off.

This carve-out is **not** a general backdoor around §6; it exists only for
security and soundness.

### 3.4 Error codes (I#157/F1)

Every diagnostic the toolchain emits carries a stable **error code** of the form
`ZAPnnnn` (`ZAP` + four digits; the leading digit groups by category/phase —
`1` lex, `2` parse, `3` name, `4` type, `5` semantic, `6` back end). The code,
**not** the human-readable message, is the machine-parseable, compatibility-
governed identifier that tools and coding agents key off. `zap explain <ZAPnnnn>`
prints a code's category, title, and meaning.

For a code marked **Stable** (see §4), within a MAJOR:

- A code's **documented meaning** — its category and the condition it identifies,
  as recorded in the code registry / `zap explain` — is **stable**. The same
  condition keeps reporting the same code.
- **Adding** a new code is a backward-compatible **MINOR** change (§1.2).
- **Renumbering, removing, or repurposing** a Stable code (making it identify a
  different condition) is **breaking → MAJOR**, after deprecation (§5).
- The **message text** attached to a code is **not** stable and may change in any
  release, PATCH included. Consumers must key off the code, never the prose.
- **Broadening** a code — making it cover more cases while its documented meaning
  still holds — is a MINOR change. **Narrowing** it — so a condition that
  previously produced the code now produces a different one — is breaking for the
  old code (it is a repurposing from that condition's point of view).
- A code's recommended **severity** may be relaxed (error → warning) at a MINOR;
  **tightening** it (warning → error) can turn a program that compiled into one
  that does not, so it is governed as a language change (MAJOR, or the §3.2
  bug-fix carve-out when the program was non-conforming).
- The `--json` / `zap mcp` diagnostic `code` field is part of that diagnostic
  schema, and per §3.1 the schema is covered only once explicitly stabilized.

**Tiering.** Error codes ship **Experimental** (§4): pre-1.0 (§2) makes no promise
and codes may be **renumbered freely** until the stable set is frozen. The
authoritative Stable set is ratified by the **#206 spec-freeze audit** at 1.0;
from that release the rules above bind every code in it.

---

## 4. Stability tiers and graduation

Every public surface sits in exactly one tier. The tier is recorded where the
surface is documented (`SPEC.md`, the decision log, the reference) and is the
contract for that surface.

- **Stable** — covered by the §3 promise. Changed only additively (§1.2) or via
  the §6 process.
- **Experimental** — shipped but **not** promised. May change or be removed in
  any release (with a `CHANGELOG` note). Intended for surfaces we want real-world
  use of before committing. A use of an Experimental surface is **opt-in and
  signposted** (see §4.1).
- **Internal** — not a public surface at all; no documentation contract and no
  promise. The runtime/ABI (§1.5), ZIR, codegen, and diagnostic command output
  live here.

### 4.1 Signposting experimental surfaces

Pre-1.0 (§2) **everything is effectively Experimental** regardless of label.

At and after 1.0, an Experimental surface must be *unmistakable* at the point of
use. The mechanism itself is implemented under the tooling track (not this
policy), with this **required behavior**: using an Experimental language feature
or `std` API is gated behind a **documented opt-in** — the design target is a
per-project `[experimental]` allow-list in `zap.toml` plus a compiler flag, and
the toolchain **warns** on any use of an Experimental surface that is not
explicitly opted into. Until that gate ships, Experimental surfaces are marked
as such in the reference and enumerated by the #206 audit, and carry **no**
promise whether or not they are reachable. "I could reach it, so it must be
stable" is never a valid assumption.

### 4.2 Graduation: Experimental → Stable

- Happens in a **MINOR** release (it is a backward-compatible addition of a
  promise, §1.2).
- Requires a **`DECISIONS.md` entry** recording the final surface and the intent
  to freeze it, and the surface must be fully documented in `SPEC.md` / the
  reference.
- From that release on, the surface is covered by §3 and may only change per §5
  and §6.

### 4.3 Demotion and removal of Experimental

- An Experimental surface may be changed or removed in any release with a
  `CHANGELOG` note; post-1.0 it does **not** get the §5 deprecation window
  (which protects **Stable** surfaces only).
- A Stable surface is **never** silently demoted to Experimental; removing its
  promise is a breaking change (§6) and happens only at a MAJOR, after
  deprecation (§5).

### 4.4 Provisional 1.0 tiering (to be ratified by #206)

The **authoritative** Stable/Experimental/Internal classification for 1.0 is
produced by the **spec-freeze audit (#206, epic Area D1)**, which this document
unblocks. The table below is the **framework and a provisional seed** for that
audit, derived from `SPEC.md` Appendix B and the decision log — **not** the
final freeze. #206 may move any row.

| Surface | Provisional tier | Note |
|---|---|---|
| Declarations, scalars, operators, control flow, functions/lambdas | **Stable** | `SPEC.md` §3–§7, Locked. |
| Strings, interpolation, conversions, value/reference semantics | **Stable** | Locked. |
| Structs, interfaces, enums (incl. data-carrying, D-0046) | **Stable** | Syntax Locked; audit to confirm sum-type surface. |
| Exceptions, `try/except/finally`, `with` / context managers (D-0069) | **Stable** | Locked. |
| Modules, imports, visibility (D-0028, D-0052) | **Stable** | Locked. |
| Generics (incl. bounds) | **Stable** (candidate) | Recently shipped; audit confirms. |
| Coroutines, channels, `select`, structured `scope:` (D-0044/45) | **Stable** (candidate) | Model Locked; some surface marked "syntax draft". |
| First-class function values + closures (D-0067 era) | **Stable** (candidate) | Recently shipped. |
| Regex (D-0010), decorators/annotations core + `@derive(Eq, Repr)` | **Stable** | Locked. |
| Core built-ins and core `std` (`io`/`fs`, `os`, `sys`, `strings`, `math`, `collections`, `time`, `json`, `regex`, `logging`, …) | **Stable** (per-module, audited) | #206 classifies module-by-module. |
| CLI: `run`, `build`, `test`, `fmt`, `new`/`init`, `add`/`remove`, `version`, `help` | **Stable** | `SPEC.md` §17. |
| `zap.toml` / `zap.lock` / resolver (D-0052) | **Stable** | Per §1.4. |
| `threading` compat module (I#175) | **Experimental** | Flagged migration-only; prefer coroutines. |
| Python interop (`import python`, D-0003/design 0003) | **Experimental** | "Rare relief valve." |
| Numeric arrays / SIMD, dtype promotion (D-0011, design 0002) | **Experimental** | Incremental; promotion rules open. |
| `set` type, `**kwargs`, user-defined macros/decorators beyond `@derive` | **Experimental** / not-yet | Open questions; ship Experimental if present. |
| C FFI (D-0012), arenas, WASM target | **Experimental** when shipped | Deferred / post-1.0. |
| `zap mcp`, `zap inspect`, `zap zenv`, `zap expand`, `zap ir` | **mixed** | Commands Stable where listed; their *output formats* Internal unless a `--json` schema is stabilized. |
| Error codes (`ZAPnnnn`) + `zap explain` (I#157/F1) | **Experimental** (Stable-audited target) | Code *meaning* stable per §3.4 once ratified; message **text** never stable. #206 freezes the Stable set. |
| ZIR (`design/IR.md`), `zaprt`/ABI (`design/RUNTIME-ABI.md`), GC algorithm, scheduler internals, codegen output | **Internal** | §1.5; never promised. |

---

## 5. Deprecation policy (Stable surfaces)

A Stable surface is **never removed without warning**. The lifecycle is:

1. **Deprecate** in a **MINOR** release. The surface keeps working and keeps its
   behavior. The deprecation is recorded in the `CHANGELOG` and — for a
   language/stdlib surface — in `DECISIONS.md`, with the replacement and a
   migration note. The toolchain emits a **deprecation warning** at the point of
   use (the planned `@deprecated` built-in, OPEN-QUESTIONS, is the mechanism for
   library surfaces).
2. **Keep** the deprecated surface functional for the **remainder of the current
   MAJOR**. Because removing a Stable surface is breaking (§1.1), it can only be
   removed in the **next MAJOR**. Therefore:
   - **Minimum window:** a surface deprecated at any point in `1.x` remains
     available (deprecated, warning) through the end of `1.x`.
   - **Removal** happens at the MAJOR boundary (`2.0.0`) at the earliest.
3. **Remove** in the MAJOR, listed in that release's migration guide (§6).

Deprecation does not by itself change behavior — it is a signal, not a break.

---

## 6. Breaking-change process (MAJOR)

A MAJOR release is the only place a Stable surface may break or be removed.
Reaching for a MAJOR is a deliberate, documented act:

- **Justification.** Each break requires a **design doc** (`design/TEMPLATE.md`)
  and a **`DECISIONS.md` entry** stating what breaks, why the compatible path
  was insufficient, and the migration.
- **Deprecate first.** Anything removed at `X.0.0` must have been **deprecated**
  (§5) in the preceding MAJOR's series wherever that was feasible; a break with
  no prior deprecation requires explicit maintainer sign-off and a `CHANGELOG`
  rationale.
- **Migration guide.** Every MAJOR ships a migration guide enumerating every
  break and the mechanical fix.
- **Automated migration (goal).** The project *aims* to provide a `zap fix`-style
  migrator that rewrites source across a MAJOR boundary where mechanically
  possible. This is a goal, not a guarantee, for the first MAJOR transitions.
- **Language-version pin (editions alternative — see §8).** A project may pin
  `language = "<major>.<minor>"` in `zap.toml`; the toolchain guarantees it
  accepts that language level within the same MAJOR and uses the pin to reject
  newer syntax and to stage opt-in migrations.

The §3.2 bug-fix and §3.3 security/soundness carve-outs are the **only** ways a
Stable surface's observable behavior changes outside a MAJOR.

---

## 7. Support and compatibility windows

### 7.1 Compatibility window

The §3 promise holds for the **entire `1.x` series** — there is no sunset within
a MAJOR (the Go model). A program targeting `1.0` is expected to build on the
last `1.x` release.

### 7.2 Patch/support window

- Only the **latest MINOR of the current MAJOR** receives routine bug-fix
  (PATCH) releases — releases roll forward.
- When a new MAJOR ships, the **previous MAJOR** continues to receive
  **security and critical-correctness** fixes for a **migration window of at
  least 12 months** from the MAJOR's release, so projects have time to migrate.
- These durations are the policy; the mechanics (branches, backports, release
  cadence) are owned by the release-engineering track (E1 / #207).

### 7.3 Platform support matrix

- **1.0 targets Linux + macOS + Windows**; WASM is a post-1.0 fast-follow (epic
  #158, `design/0016-platform-1.0.md`). The authoritative, per-architecture
  matrix (tier-1 CI-gated vs tier-2 best-effort) lives with the platform work.
- **Adding** a platform/architecture is a backward-compatible **MINOR** change.
- **Dropping** a platform/architecture from the supported matrix is **breaking**
  → MAJOR (it can make a previously-buildable project unbuildable).

### 7.4 Build-time dependencies (MSRV, LLVM)

The versions needed to **build the compiler from source** (the Rust MSRV,
currently `rust-version = 1.74`; the LLVM version, currently 18) are **not** a
Zap-program surface — they do not affect whether a `.zap` program compiles or how
it behaves. Therefore:

- Raising the MSRV or the required LLVM major is a **MINOR** change, announced in
  the `CHANGELOG`. It is never a Zap-language break.
- The supported build-dependency versions are stated in `README.md` /
  `CONTRIBUTING.md` and tracked by CI.

---

## 8. Editions — rejected for 1.0

**Decision: Zap does not adopt a Rust-style edition mechanism for 1.0.**

Rust needs editions because crates compiled under different editions must link
together in one binary, so the edition is a per-crate property the compiler
threads through. Zap compiles **whole-program, ahead-of-time** (D-0021): there is
no cross-edition linking problem to solve, and a full edition system would add
real complexity (per-unit language modes, cross-edition interop rules) for
little gain in Zap's model.

Instead Zap follows the **Go model**: one language that evolves
backward-compatibly within a MAJOR (§3), with breaking changes batched into the
next MAJOR (§6). The lightweight **`language = "<major>.<minor>"` pin** in
`zap.toml` (§6) covers the one case editions are really wanted for — letting an
older project keep its language level and opt into migrations on its own
schedule — without the machinery of true editions.

This decision is revisited only if a concrete need appears post-1.0 that the
language pin cannot meet; any reconsideration goes through the design-doc +
`DECISIONS.md` process.

---

## 9. Enforcement

This policy is a set of gates, not guidance:

- **Every PR that changes a public surface** must (a) add a `CHANGELOG` entry and
  (b) state the change class — PATCH / MINOR / MAJOR — by the §1 rules. A PR that
  changes a **Stable** surface incompatibly is **blocked** unless it is a
  sanctioned §3.3 security/soundness fix with sign-off, or lands on a MAJOR
  branch under §6.
- **A `DECISIONS.md` entry is required** for: graduating Experimental → Stable
  (§4.2), deprecating a Stable surface (§5), and any MAJOR break (§6).
- **The release process (#207)** verifies that the version bump equals the
  maximum change class since the last release, and that pre-release tags (§2)
  are used for anything not yet frozen.
- **The #206 spec-freeze audit** is the source of truth for the Stable set at
  1.0; this document defines the tiers and the rules that govern them (§4).
