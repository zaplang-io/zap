# Zap code-intelligence index — design

Authoritative design for `zap mcp`'s code-graph surface and the index layer
behind it, from the current in-memory implementation up to fleet-/worldwide-scale
(large monorepos × many concurrent agents). Supersedes the earlier
`zap-mcp-budget-cache-design.md` (its budget + cache sections are folded in here,
§4–§6).

Everything here is **dep-free** (hand-rolled `Json`, `std` only) unless a section
explicitly flags a dependency as a weighed cost. Each top-level section is sized
to become one ticket (see §15).

---

## 0. Problem & scope

`zap mcp` exposes a code graph (`zap_code_graph`) so agents get a token-efficient
map of a project instead of reading raw files. It works well at the current repo
scale but has three unbounded axes:

1. **Output size** — a no-filter query returns every symbol, rendered twice.
2. **Compute** — the whole index is rebuilt (full parse) on every call.
3. **Memory at scale** — one in-RAM index per session process; on a large
   monorepo × many concurrent worktree sessions this multiplies into GBs of
   near-identical data.

Zap ships worldwide, so the design must solve for large monorepos and many
agents, not just a single small repo. The organizing decision is a **change of
indexing unit** (§3) that makes the rest fall out cleanly.

## 1. Current state (grounded)

- `crates/driver/src/mcp/` — hand-rolled JSON-RPC 2.0 over newline-delimited
  stdio. Tools: `zap_reference`, `zap_check`, `zap_code_graph`, `zap_test`,
  `conventions`.
- `Server { root: PathBuf }` — **stateless**; all handlers `&self`; `run()` loops
  stdin lines **sequentially, single-threaded** (`mod.rs:85`).
- `tool_code_graph` rebuilds `graph::Index::build(&root)` on **every** call
  (`mod.rs:286`); `Index::to_json` emits **both** a structured `results` array
  **and** a `packed` text block (`graph.rs:127`) — double encoding.
- `Symbol` holds signatures only, no bodies; each `Symbol` stores its own
  `file: String` — **path duplicated per symbol** (`graph.rs:29`).
- `check_source(src)` is pure on input (`check.rs:70`).
- `conventions` already caps its body at 32 KiB with truncation-on-notice
  (`conventions.rs:84`) — the pattern to reuse.
- Measured scale today: 188 `.zap` files, ~4.75k lines, ~165 KB → index builds in
  single-digit ms. Memory is not a *current* problem; it is a *shipping* problem.
- Adjacent machinery to reuse: `crates/pkg` (packages, `zap.toml`), registry
  (I#236, checksums), `zap publish` (#237), doc extract/render (#232/#233),
  `conventions` (#108), `zap lsp` (#228–#231).

## 2. Design principles

- **Emit the minimal faithful representation at the source** — we own the AST, so
  never ship bytes we don't need (signatures not bodies), never compress after the
  fact.
- **Fidelity to the checkout is sacred** — the graph must reflect exactly the
  files visible to the serving process; never serve symbols the consumer's own
  compiler can't see (§13).
- **Unit of indexing = unit of the build system** — the package, not a directory
  tree (§3). This is how rust-analyzer (per-crate), Bazel/Buck (per-target), and
  Glean (per-unit) scale.
- **Immutable keys where possible** — commit SHAs and published versions are
  content-stable, so they make staleness-free cache keys.
- **Verify, don't trust** — any precomputed/third-party artifact is a cache of
  something re-derivable, validated against source; author prose is data, never
  instructions (§10).
- **One index layer, many frontends** — `zap mcp`, `zap lsp`, `zap doc` share it
  (§11).

## 3. The model shift: package-segmented index (the keystone)

**Decision:** the graph is a *set of per-package segments* composed on demand via
the dependency graph — not one flat index of a directory tree.

- The **package** (a `zap.toml` member / workspace unit) is the indexing unit.
- A project's effective graph = its working package ⊕ the segments of its
  transitive dependency closure.
- A monorepo is *already* partitioned into service/lambda/lib packages with
  declared deps; ride that partition.

Why this is the keystone — it collapses all three axes at once:

| Axis | Flat whole-tree index | Package-segmented |
|---|---|---|
| Huge monorepo | loads everything | loads working package + dep closure only |
| Many sessions | N copies of ~everything | shared library packages indexed **once**, referenced by all |
| Cold start / incremental | re-parse all | edit package A → only A re-indexed; stable deps are hits |
| Relevance | agent sees 400k symbols | default scope = current package; widen on request |

In a monorepo the shared-library overlap is large, so segmenting along package
boundaries is exactly where the wasted duplication was.

**Query semantics:** `zap_code_graph` defaults to the **current package**
(resolve `root` → nearest `zap.toml`), with opt-in to widen to the dependency
closure or whole workspace. `root`-subdir scoping already exists (`mod.rs:282`)
and is the v1 approximation of this.

## 4. Output budget + pagination (`zap_code_graph`, then check/test)

**D1 — stop double-encoding.** Add `format`: `packed` (default) | `json` |
`both`. `packed` is token-efficient; `results` repeats keys per symbol (~2–3×).
Default `packed` ≈ halves output. **One visible behavior change** (default was
effectively `both`); flag in PR, `format=both` is back-compat.

**D2 — hard byte budget, truncate on a symbol boundary.** `GRAPH_BUDGET` default
≈ 24 KiB of rendered body; `max_bytes`-overridable, clamped `[4,64] KiB`. Build
symbol-by-symbol; stop before the next *top-level* symbol (with children) exceeds
budget — never cut mid-symbol. Append a visible notice (mirror `conventions::cap`).

**D3 — stable, self-validating cursor.** Files sort deterministically, source
order within → the filtered match list is a stable sequence. Cursor =
`(fingerprint, offset)`, encoded dep-free as `format!("{fp:x}.{off}")`. If the
cursor's fingerprint ≠ current tree, restart from 0 with `cursor_reset: true`.
Emit `next_cursor` only when truncated.

**D4 — budget governs the dominant rendering only.** Counts + `shown` +
`next_cursor` + `truncated` always included.

**check/test follow-ons:** `zap_check` gets `max_diagnostics` (default ~100;
already source-sorted). `zap_test` caps per-test failure-output bytes + total
payload, keeps counts exact, points at single-file run for full output.

Wire (all optional, additive): `max_bytes`, `cursor`, `format` on
`zap_code_graph`. Code: `render_budgeted(matches, offset, max_bytes, format) ->
(Json, Option<next_offset>)` replaces the body half of `to_json`; keep `to_json`
as the unbounded `both` case for existing tests.

## 5. Per-session cache + `known_hash`

**State:** `Server` is `&self`, single-threaded → add
`cache: std::cell::RefCell<SessionCache>`. No signature churn. (Swap to `Mutex`
only if a threaded/socket transport arrives — §9.)

```
struct SessionCache {
    graph: Option<GraphEntry>,       // { root, fingerprint, index }
    check: LruMap<u64, String>,      // source-hash -> rendered check JSON (cap ~64)
}
```

- **Graph cache:** key `(root, fingerprint)` (§6). Hit → reuse parsed index, skip
  all parsing. Miss → rebuild. Stat-walk ≪ parse, so this is the main compute win.
- **Check cache:** key `hash(source)` (inline) or `hash((path,mtime,len))`.

**`known_hash` (the token win, not just CPU):** optional arg on
`zap_code_graph`/`zap_check`. Server computes the hash of the body it *would*
return; if `known_hash` matches → `{ "unchanged": true, "hash": "<h>" }` (bytes,
not the body). Else return body + top-level `"hash"` the agent echoes next call.
This is "re-read returns a diff," precise because we own parsing. (v2: return a
changed-symbols delta.)

## 6. The shared `fingerprint` primitive

`graph.rs::fingerprint(root) -> (Vec<PathBuf>, u64)`: stat-walk via `collect_zap`,
hash ordered `(path, mtime, len)` with `std::collections::hash_map::DefaultHasher`
(no dep, MSRV-safe). Returns the file list so `build` reuses it (walk once).
**This one `u64` is used everywhere:** the pagination cursor (§4 D3), the session
cache key (§5), segment keys (§8), and baseline validation (§7). A cursor is
valid iff its fingerprint matches the cached entry — so cursor validation *is* a
cache lookup. Build it once.

## 7. Persistence: commit-pinned baseline ⊕ working delta

**Decision:** do not persist mutable working trees; persist only the **committed
baseline**, which is immutable per commit.

- Baseline = graph of the tree at a commit SHA → key by SHA, **never stale**.
- Effective graph for a worktree = `baseline(merge-base SHA)` ⊕ local working
  delta (changed/added files via per-file `(path,mtime)` fingerprint).
- The stable bulk (stdlib + crates on `main`) is shared across all worktrees; only
  the churny branch delta is per-checkout.

This is the only coherent cross-session reuse. A single *mutable* shared graph is
incoherent and actively harmful (§13). Per-package (§3), the baseline is itself a
set of commit-pinned package segments (§8).

## 8. On-disk segment store + working-set eviction

For large repos, bound *per-process* resident memory regardless of repo size.

- **Per-package segments on disk**, each keyed `(package-id, content-fingerprint)`
  — commit-pinned segments are immutable.
- **Seekable binary format** (fixed-layout records + a **string table**) so
  individual symbols load without materializing the whole segment. The string
  table also fixes the per-symbol path duplication (§1) — do path-interning here
  even in the pure in-memory form as a cheap early win.
- **Manifest** maps `(package, fingerprint) → segment file`.
- **Closure loading:** resolve the working package's deps (via `crates/pkg`), load
  only those segments.
- **Working-set eviction:** a bounded LRU of resident segments; drop cold ones.
  The *package* is the eviction unit ("hot paths in memory," cleanly).
- Dep-free with plain `File`+`seek` (bounds per-process RAM to the working set);
  cross-process resident dedup needs §9.

## 9. Cross-session dedup: mmap vs. daemon (one open decision)

Two ways to stop N sessions from each holding the shared baseline:

- **(a) mmap segments read-only** → OS page cache holds **one physical copy** of
  touched pages across all N processes. Keeps the one-process-per-session stdio
  model. **Cost:** a dep (`memmap2`) or unsafe `libc` — against the no-deps ethos.
- **(b) one shared daemon** over the socket transport (deferred in D-0055,
  `cli.rs:340`): a single process holds **one** in-RAM baseline for all worktrees
  + per-worktree deltas. **Cost:** daemon lifecycle + per-request root handling;
  **no format/mmap dep**, dedup is just "one process instead of N."

**Recommendation:** if the socket transport is on the roadmap anyway, prefer (b)
— cleaner, dep-free, folds the baseline-overlay into one in-RAM structure. Decide
when the transport lands. Note: per-session processes already free their memory
on exit, so pressure is bounded to *concurrent* sessions.

## 10. Shipped library graphs (publish + registry) + trust model

**Decision:** a published library ships a precomputed graph segment (Zap's
`.d.ts`/rustdoc-JSON/`.rmeta` analogue); consumers load it instead of parsing the
dependency.

- **Generation:** `zap publish` (#237) emits the segment as a sibling artifact
  (same `graph.rs` logic, format-versioned). Registry (I#236) stores it with its
  checksum.
- **Consumption:** for each dep, `zap mcp` uses the shipped segment if present,
  else builds locally (§8). Keyed by the **immutable published version** →
  best-possible dedup: built once at publish, consumed worldwide; no consumer ever
  parses the dependency.

**Two halves, different trust:**
1. **Structural graph** (signatures/symbols) — *derivable from source.* Make it
   **trust-free**: where the consumer has the dep's source, treat the shipped
   segment as a **cache and validate** against the source fingerprint; on mismatch
   **discard and rebuild**. It is only ever a performance cache of something
   verifiable.
2. **Curated prose** (author examples, "prefer this API", deprecations) — *not*
   derivable; the real value for agents. Treat as **data, clearly attributed**
   ("from library X"), **never as instructions**; keep in a separate labeled
   channel from the structural graph (per-library sibling of `conventions` #108).

**Provenance ≠ benignity:** registry checksums prove the artifact is authentic,
not safe — hence regenerate-or-check for structure, data-not-instructions for
prose. **Source-absent** (binary-only) libs: trust the declarations like `.d.ts`;
the prose rule still holds. Immutable version key ⇒ no staleness; format skew ⇒
graceful rebuild-from-source.

**This is a supply-chain injection surface.** It is safe *only* with the
verify/attribute discipline above; that discipline is the crux of the feature.

## 11. Shared with `zap lsp`

The segmented package-index store is **not MCP-only**. `zap lsp` (#228–#231) needs
the same: per-package symbol index, incremental on edit, dependency-aware. Build
it **once** as the toolchain's code-intelligence layer and serve `zap mcp`,
`zap lsp`, `zap doc`, and registry tooling from it (rust-analyzer's model: one
index, many frontends). This reframes the work from "an MCP optimization" to
"shared infra," and is the main argument for investing in §8 properly.

## 12. Deployment / per-worktree registration

- `zap mcp` is spawned **by the client**, once per session, with cwd = the
  session folder. Users never launch it by hand.
- **Commit `.mcp.json` at the repo root** (Claude Code project scope) — a tracked
  file is present in every worktree checkout automatically, so each session
  auto-spawns `zap mcp` with the right cwd. One file registers the fleet. Cursor
  et al. use the same shape (`.cursor/mcp.json`), equally committable.
- **Make root resolution robust:** when `--root` is absent, walk up from cwd to
  the nearest project marker (`.git` / `zap.toml`) instead of bare
  `current_dir()`, so launch-cwd quirks don't mis-scope the index.
- Friction to document: Claude Code asks a one-time **trust/enable** per project
  for `.mcp.json` servers (a click, not a launch); `zap` must be on PATH.
- Optional `zap mcp init [--client …]` to generate configs for clients not covered
  by the committed file (mirrors mcplens's `init`).

## 13. Cross-session correctness (why NOT to share unlanded work)

Each worktree is a different checkout (different branch + uncommitted edits). The
graph's contract is fidelity to *that* checkout. Sharing a mutable graph across
worktrees would serve **phantom symbols** — e.g. a symbol on branch A fed to a
session on branch B whose compiler cannot resolve it. That is worse than no cache.

So: "unlanded work from a sibling branch" is precisely what the graph must
**exclude**. Cross-session *awareness* (what other sessions/branches are doing) is
real but is **coordination metadata**, not a code index — it belongs in the
orchestrator / notes / PR state, never merged into the compiler-truth surface.
The only sound cross-session reuse is the **committed, immutable baseline** (§7).

## 14. Memory & cold-start math (why the above is justified)

- Per symbol ≈ 200–300 B (signatures only). Density ~1 top-level symbol / 15–30
  lines + members.

| Repo | ~Symbols | ~Index RAM/process |
|---|---|---|
| Today (4.75k lines) | ~300–600 | <1 MB |
| 500k lines | ~20–40k | ~10 MB |
| 5M lines | ~200–400k | ~50–100 MB |

- 30 concurrent sessions × 50 MB huge-repo index ≈ 1.5 GB of mostly-duplicate
  data → the case §3/§8/§9 remove.
- Cold-start build is **parse-only** (no sema/LLVM): ~5–50 MB/s, so sub-second to
  a few seconds even for very large trees; the repeated per-call rebuild (fixed by
  §5) is what actually bites in a loop.
- Optional: **parallelize** `Index::build` (embarrassingly parallel `std::thread`
  scatter/gather) and/or **background-warm** the first build after `initialize`,
  if cold start ever becomes user-visible on a huge single package.

## 15. Staging → tickets

Ordered; each maps to ≈ one ticket. Early items are independent and low-risk.

1. **Path-interning + `render_budgeted`** — string table in `graph.rs`; budget +
   `format` default `packed` (§4 D1/D2/D4, §8 string-table early win). Behavior
   change flagged.
2. **Pagination cursor + `fingerprint()`** — §4 D3 + §6. Depends on 1.
3. **check/test budgets** — `max_diagnostics`, test-output caps (§4 follow-ons).
4. **Session cache + `known_hash`** — §5, reuses §6. Depends on 2.
5. **Deployment** — commit `.mcp.json`, root walk-up, optional `zap mcp init`
   (§12). Independent; can land early.
6. **Package-scoped querying** — default to current package, resolve
   `root`→`zap.toml`, widen on request (§3 query semantics). Depends on `pkg`.
7. **On-disk segment store** — seekable format + manifest + closure loading +
   LRU eviction (§8). The big one; shared-infra investment (§11).
8. **Commit-pinned baseline ⊕ delta** — §7, on top of 7.
9. **Cross-session dedup** — pick mmap vs. daemon (§9) once socket transport
   status is known.
10. **Shipped library graphs** — publish emits segment, registry stores, consumer
    verifies/merges, trust model (§10). Depends on 7 + registry/publish.
11. **`zap lsp` shares the store** — §11; after 7 stabilizes.

Near-term value (1–5) ships against the current architecture and solves the
output/compute/UX problems now. 6–11 are the worldwide-scale build-out.

## 16. Open decisions

- **§9 mmap vs. daemon** — gated on whether the socket transport lands (D-0055).
  Leaning daemon.
- **§4 D1 default** — `packed` (recommended) vs. `both` for strict back-compat.
- **§10 curated-prose channel** — separate tool/surface vs. a labeled field in the
  segment; and how strongly to sandbox it.
- **§8 format** — bespoke fixed-layout vs. a minimal existing container; must stay
  dep-free and seekable.
