# Proposal: Package registry — index, checksums, naming, publish

- **Author(s):** Zap toolchain team
- **Last updated:** 2026-10-25
- **Status:** Draft
- **Discussion:** issue #235 (registry track of epic #157, Area D); builds on
  D-0052 / `design/0009-packaging.md`; constrained by D-0070
  (`docs/STABILITY.md`) and D-0071.

> Formatting convention (from Go): wrap near the 80-column mark and start each
> sentence on a new line.

## Abstract

This proposal designs an **optional** package registry for Zap: a shared index
of package names, versions, and metadata, a shared checksum database, and
hosted package archives.
It is strictly additive to the decentralized model already shipped in D-0052 —
`git` and `path` sources keep working unchanged, and there is still **no
mandatory central registry**.
The registry adds a fourth dependency source (a short name resolved through an
index) plus the "optional official caching proxy" and shared `zap.sum` that
D-0052 and `design/0009-packaging.md` explicitly deferred.

The recommendation is a **static-file HTTPS index** (a sparse, per-package index
served as plain files from object storage behind a CDN), **tree-hash
integrity** reusing the existing `zap1:` content hash so a lockfile pin is
identical across `git` / `path` / `registry` sources, **flat names with an
optional `@scope/` namespace**, **immutable versions with Go-style yank** (never
delete), and a token-based first-publisher ownership model for `zap publish`.

This is a **design-only** ticket.
It specifies what #236 (index + checksum client) and #237 (`zap publish`)
implement.
Nothing here is implemented in this ticket, and the hosting itself is a
deployment decision gated on the zaplang-io org move (§8, §9).

## Background

### What exists today (D-0052, PR #96, `design/0009-packaging.md`)

The package manager is built and in `main`:

- **`zap.toml`** — author-edited manifest.
  A `[dependencies]` entry is a table with exactly one source key plus an
  optional version: `git = "<url>"` + `version = "<semver>"`, or `path =
  "<dir>"`.
  The table key is the **logical name** bound by `import`.
- **`zap.lock`** — tool-generated, committed.
  Each `[[module]]` row records `name`, `version`, `source` (`git+<url>` or
  `path+<dir>`), and `checksum = "zap1:<hex>"`.
- **Resolver** — Go-style Minimal Version Selection (MVS).
  Module identity is `(name, major)` so majors coexist; a version requirement is
  a **minimum**, and the resolver selects the maximum of the minimums demanded
  across the graph.
  The resolver is pure and generic over a `Registry` trait
  (`crates/pkg/src/resolver.rs`):

  ```rust
  pub trait Registry {
      fn requirements(&self, name: &str, version: &Version)
          -> Result<Vec<Requirement>>;
  }
  ```

- **`checksum`** — a self-contained SHA-256 (`crates/pkg/src/checksum.rs`,
  `PREFIX = "zap1:"`) over a module's **tree**: the hash of the sorted
  `relpath\0sha256(bytes)\n` lines of every file in the module.
  This digest is content-addressed and **transport-independent** — it depends
  only on the files, not on how they were delivered.
- **`zenv` / `ops`** — a project-local cache (`$ZAP_ZENV/pkg`) materialized from
  `zap.lock`, reached through `module_search_paths()`.

The distribution stance is locked (D-0052): dependencies resolve to VCS sources,
integrity comes from a checksum database, and a central registry is **not**
mandatory.
`design/0009-packaging.md` "Open issues" defers exactly three things this
proposal picks up: a shared `zap.sum` / official caching proxy, computing the
checksum of a remote source at fetch time, and the index that lets a short name
resolve to a source.

### What the registry is for

The decentralized model works but has the known VCS-registry gaps: no name →
source discovery, no central integrity record (each project holds only its own
slice of the checksum DB), and dependence on third-party git hosts staying up.
The registry closes those gaps **without** becoming a chokepoint: it is one more
`Registry` implementation feeding the same MVS resolver, and every artifact it
serves is verified client-side against the same `zap1:` hash the lockfile
already pins.

### The constraints that bind this design

- **D-0070 / `docs/STABILITY.md`.**
  The toolchain is one SemVer 2.0.0 artifact; a release's bump is the max over
  surfaces.
  The **package format** is a stable surface: `zap.toml`, `zap.lock`, the
  module-cache and checksum-DB layout, and the resolver's observable behavior.
  §1.4 is the governing rule — new keys/fields are additive and ignorable
  (MINOR); a change that makes an older `1.x` tool misresolve or refuse a
  newer-tool project is MAJOR.
  Pre-1.0 (§2) makes no promise, so the registry ships **Experimental** and its
  format may change freely until the #206 spec-freeze audit.
- **D-0071.**
  Any diagnostics the registry client emits (checksum mismatch, name conflict,
  yanked selection) carry stable `ZAPnnnn` codes from the `zapc_diag` registry,
  back-end category (leading `6`), and are governed by the error-code policy.
- **Org move.**
  The official org is **zaplang-io** and the domain is **zaplang.org** (Route
  53); code is still at `parveshgarg/zap` and the transfer is deferred until the
  active sessions are quiet.
  Hosting therefore must be nameable without assuming the move has happened
  (§8).

## Proposal

### 1. Index model — a static-file HTTPS index

**Recommendation: a sparse, per-package index served as plain static files over
HTTPS, behind a CDN.**
There is no index server and no index git repository: the index is a tree of
immutable-ish JSON files in object storage, and the client fetches exactly the
one file for each name it needs to resolve.

#### 1.1 Layout

The index root is a single HTTPS base URL (§8).
Each package has one index file at a path derived from its **normalized name**
(§3), using a short fan-out prefix so no directory holds the whole namespace:

```
<index-base>/
  config.json                 # index metadata (see §1.3)
  3/j/jsonlib.json            # 1–3 char names: len-bucketed dirs
  ma/th/mathx.json            # >=4 chars: first two / next two chars
  @acme/                      # a scope is a directory
    wi/dg/widgets.json
```

The fan-out mirrors Cargo's sparse index (`ca/rg/cargo`) so the scheme is
familiar and collision-free; the exact bucketing is recorded in `config.json`
so the client never hard-codes it.

#### 1.2 Index entry schema

A package file is **line-delimited JSON** (one object per published version, in
publish order), so appending a new version is a pure append and old lines are
never rewritten:

```json
{"name":"jsonlib","vers":"1.4.0","deps":[{"name":"bytesx","req":"0.3.0","major":0}],"cksum":"zap1:9f86…","yanked":false,"min_zap":"1.2.0","edition":"2026"}
{"name":"jsonlib","vers":"1.5.0","deps":[{"name":"bytesx","req":"0.3.0","major":0}],"cksum":"zap1:a1b2…","yanked":false,"min_zap":"1.2.0","edition":"2026"}
```

Fields:

| Field | Meaning |
|---|---|
| `name` | normalized registry name (§3). |
| `vers` | the published SemVer version (exact, one row per version). |
| `deps` | this version's own dependencies: `{name, req, major}` where `req` is the **minimum** SemVer (MVS) and `major` carries the `(name, major)` identity so the client need not re-parse. A `deps` entry may also carry `registry` when a dep comes from a *different* index (§8.3). |
| `cksum` | the `zap1:` tree hash of the package content (§2) — the same value the lockfile pins. |
| `yanked` | boolean; a yanked version is excluded from new resolutions but stays resolvable (§4). |
| `min_zap` | minimum toolchain version that can build this package (SemVer); the client refuses a package whose `min_zap` exceeds its own version with a clear code. |
| `edition` | optional language edition tag; omitted until editions exist, reserved now so adding it later is additive (§1.4). |

Unknown fields are **ignored** by older clients (D-0070 §1.4), so the schema
grows at MINOR.
`yanked` flips in place: that is the **only** mutation to an existing line, and
it is monotonic in practice (a yank can be reversed by a re-publish decision but
the `(name, vers, cksum)` triple is immutable).

#### 1.3 `config.json`

```json
{"index_version":1,"dl":"https://<archives-base>/{name}/{name}-{vers}.zap.tar.zst","api":"https://<api-base>","bucketing":"cargo-sparse"}
```

`dl` is the archive URL template, `api` is the (publish-only) write endpoint
(§5), and `bucketing` names the fan-out scheme.
Keeping the download and API locations in `config.json` is what makes the org
move a config change, not a format change (§8).

#### 1.4 Why static-file, and not the alternatives

| Option | Verdict |
|---|---|
| **Git-based index** (crates.io ≤2022, Go-ish) | Rejected. A clone-and-pull index grows unboundedly, forces every client to fetch the whole namespace to resolve one name, needs a git host, and makes the index itself a mutable repo to protect. Wrong fit for minimal, not-yet-launched infra. |
| **Dynamic API index** (an always-on resolver service) | Rejected for the index read path. It needs a running, scaled, monitored service and a database; it is a single point of failure and a cost center before launch. We still define a *write* API (§5), but reads must not need it. |
| **Static-file sparse HTTPS index** (recommended) | **Chosen.** Pure files in object storage behind a CDN: zero server logic to run or secure, trivially cacheable and mirrorable, scales by CDN, costs ~nothing idle, and the client fetches O(dependencies) small files, not the whole index. It is the crates.io-today model reduced to its static essence, and it matches the "minimal-infra, not-yet-launched" posture directly. |

The static index is also the natural shape for the org move: a static tree can
be re-hosted or fronted by a new domain with a redirect, with no service
migration (§8).

### 2. Integrity and checksums

**The canonical integrity identity stays the existing `zap1:` tree hash.**
This is the single most important consistency decision in this proposal: a
lockfile pin is byte-identical whether a module came from `git`, `path`, or the
registry, because the hash is over the **content tree**, not the transport
(§Background).
The registry does not introduce a second, source-specific checksum notion into
the lockfile.

#### 2.1 What is hashed, and the archive

- **Pinned / verified value:** the `zap1:` SHA-256 tree hash — the hash of the
  sorted `relpath\0sha256(bytes)\n` lines of the package's files, exactly as
  `crates/pkg/src/checksum.rs` computes it today.
- **The archive** is a transport container: a deterministic
  `tar` + `zstd` of the package tree (`<name>-<vers>.zap.tar.zst`), built with
  sorted entries, fixed mode/mtime/uid/gid, and no VCS/junk files, so the
  archive bytes are reproducible.
  The archive carries a **secondary** digest — the SHA-256 of the archive
  bytes — recorded in the index only as a cheap pre-unpack transfer check,
  **not** as the lockfile identity.
  After download the client unpacks, recomputes the `zap1:` tree hash over the
  unpacked tree, and verifies **that** against the lockfile.
  So a different-but-equivalent archiver, or a re-compression, cannot cause a
  false mismatch: identity is the content, not the container.

#### 2.2 Lockfile pinning and client verification

`zap.lock` is unchanged: `checksum = "zap1:<hex>"` already pins it, and a
registry source reuses that field verbatim.
On fetch the client:

1. resolves (name, version) → `cksum` from the index;
2. downloads the archive from `dl`;
3. checks the archive-bytes digest (cheap, pre-unpack);
4. unpacks and recomputes the `zap1:` tree hash;
5. verifies it equals the index `cksum` **and**, if the module is already in
   `zap.lock`, equals the locked value; a mismatch is a hard error
   (`ZAPnnnn`, §D-0071) and the module is discarded.

The first time a (name, version) enters the lockfile, the index `cksum` is
trust-on-first-use recorded; from then on the lockfile is authoritative and a
later index/mirror change cannot alter what a checkout builds.

#### 2.3 Shared checksum database (`zap.sum`) and tamper model

Today each project holds only its own `(name, version) → checksum` rows.
The registry publishes the authoritative `cksum` in the index, which **is** the
shared checksum database D-0052 deferred — no separate `zap.sum` file format is
required, because the index line already carries the hash.
(A separate `zap.sum` transparency log — append-only, signed, cross-checked —
is a future hardening, noted in Open issues; it is not needed for #236.)

Tamper / compromise considerations:

- **Compromised mirror or CDN.**
  Cannot alter a build: the lockfile pins the tree hash, verified client-side
  after unpack.
  The worst a tampered mirror can do is deny service or serve a *yank*
  state the client cross-checks against the lockfile.
- **Compromised index (pre-first-pin).**
  A bad `cksum` for a never-before-seen version is the real exposure.
  Mitigations: HTTPS/TLS to the index; immutable `(name, vers, cksum)` (a
  published version's hash never legitimately changes, so any change is
  detectable and refused); and the future signed transparency log.
- **Compromised archive store.**
  Covered by 2.2 step 4 — the content hash is recomputed, so swapped archive
  bytes are caught.
- **Hash agility.**
  The `zap1:` prefix is a version tag; a future `zap2:` (e.g. SHA-512 or a tree
  hash change) can be introduced additively, with clients accepting both during
  a migration window (format MINOR per §1.4).

### 3. Package identity and naming

#### 3.1 Logical name vs registry name

These are **distinct** and must stay so.
The `zap.toml` table key is the **logical name** — the local alias `import`
binds (`jsonlib`, `jsonlib as j`).
The **registry name** is the global identifier under which a package is
published.
A registry dependency therefore names both:

```toml
[dependencies]
# logical "json" bound locally, resolved from the registry package "jsonlib":
json   = { registry = "jsonlib", version = "1.4.0" }
# scoped name:
widget = { registry = "@acme/widgets", version = "0.3.0" }
# a non-default index, named in [registries] (see §8.3):
intl   = { registry = "icu", version = "2.0.0", index = "corp" }
```

This adds one new **source key**, `registry`, to the `Source` enum
(`Source::Registry { name, version, index }`) alongside `Git` and `Path`.
Its `version` is a **minimum** SemVer, exactly like `git`'s, so MVS is
unchanged.
Its `zap.lock` `source` string is `registry+<index-id>#<name>` (default index
id elided: `registry+jsonlib`), parallel to `git+…` / `path+…`.
Omitting the source keeps the existing two-source behavior intact (additive,
§1.4).

#### 3.2 Naming rules and normalization

- **Flat names** are the base case; an optional single-level **`@scope/`**
  namespace is reserved (scopes map to orgs — see §8).
  No deeper nesting.
- A bare name matches `^[a-z0-9]([a-z0-9_-]{0,63})$`; a scope matches the same
  shape with a leading `@`, as `@scope/name`.
- **Normalization** for uniqueness and anti-squatting: compared
  case-insensitively, with `-` and `_` folded together and runs collapsed, so
  `JsonLib`, `json_lib`, and `json-lib` are **the same name** and cannot be
  separately registered.
  The index stores the normalized form; the manifest may spell it in any
  equivalent casing.
- Reserved: names colliding with built-ins / `std` module names, and a small
  reserved list (e.g. `std`, `zap`, `python`), are refused at publish.

#### 3.3 Squatting considerations

- First-publisher ownership (§5) plus normalization (3.2) closes the obvious
  typo/case variants.
- No automatic name reservation without a published version (publishing an empty
  shell to hold a name is discouraged; a future policy may reclaim long-dormant
  unpublished holds — Open issues).
- Scopes are the real anti-squat lever: an org publishes under `@org/…`, which it
  controls wholesale, so the valuable namespace is the scope, granted once.

### 4. Versioning and yank semantics

Tied directly to D-0070.

- **Requirement syntax** is unchanged: `version = "<semver>"` in `zap.toml` is a
  **minimum**, and resolution is MVS over `(name, major)` — the maximum of the
  minimums demanded across the graph.
  No ranges, no `^`/`~` operators, no "latest that fits": that is the deliberate
  D-0052 / Go design and the registry does not reopen it.
- **Published versions are immutable.**
  A `(name, version)` is published once with one `cksum`; it is never re-uploaded
  with different content.
  This is required by §1.4 (an older tool must resolve a project the same way
  forever) and by lockfile reproducibility.
- **Yank** sets `yanked: true` on the index line.
  A yanked version:
  - is **excluded from new resolutions** — MVS skips it when computing the
    max-of-minimums for a fresh lock;
  - **remains downloadable and verifiable** if an existing `zap.lock` already
    pins it, so existing checkouts keep building;
  - produces a **warning** (`ZAPnnnn`) if a manifest's minimum names a yanked
    version as the selected one, telling the user to raise the minimum.
  Yank is reversible (un-yank is a publish-side decision); it is **not**
  deletion.
- **Deletion is not offered** as a routine operation.
  Removing `(name, version)` bytes breaks every lockfile that pinned it and
  every transitive consumer — it violates the Go-1 promise (§3) for the
  package format.
  A narrow **takedown** path exists only for legal/security necessity (malware,
  DMCA, leaked secret): it removes the archive but leaves a tombstoned index
  line (version present, `yanked:true`, archive gone) so the hole is explicit
  and a
  pinned build fails loudly rather than silently resolving to nothing.
  Takedown policy is a governance decision for the org move, not a client
  feature.

### 5. Publish flow — spec only (implemented in #237)

`zap publish` end-to-end:

1. **Package.**
   Build the deterministic archive (§2.1) from the project tree, excluding
   `zenv/`, VCS dirs, and anything matched by an ignore list; refuse if the
   manifest lacks `[package].name`/`version` or the version is already
   published.
2. **Checksum.**
   Compute the `zap1:` tree hash and the archive-bytes digest.
3. **Authenticate.**
   Send a bearer **token** (from `zap login`, stored outside the repo, e.g.
   `$ZAP_CACHE/credentials`) to the index `api` endpoint over HTTPS.
4. **Upload.**
   `api` validates ownership, name rules (§3), and that `(name, version)` is new
   and immutable, stores the archive in the archive bucket, and **appends** the
   index line (§1.2).
   Upload is transactional at the line level — a version is either fully
   present (archive + index line) or absent.
5. **Index update.**
   The appended line is published to the static index; CDN invalidation for that
   one file makes it visible.

#### 5.1 Ownership / auth model (high level)

- **First publisher of a name owns it.**
  The first successful publish of a bare name (or the first claim of a `@scope`)
  binds ownership to that account.
- Owners may **publish new versions, yank/un-yank, and add or remove
  co-owners**.
  A scope owner controls all names under the scope.
- Tokens are per-account, revocable, and scopable to publish-only; they never
  enter a repo or a lockfile.
- The **trust boundary**: the `api` write endpoint and the account/token store
  are the trusted, authenticated components; the index and archive read paths
  are untrusted-transport and made safe by client-side content verification
  (§2).
  So a registry operator is trusted for *namespace integrity* (who may publish a
  name) but **not** for *content integrity* (which the lockfile pins).

#### 5.2 Gating

The write API, the token store, and the archive bucket are real hosted,
authenticated infrastructure.
They land in **#237**, which is therefore the **last** registry ticket and is
**gated on the org move** (§8) — it cannot be deployed against final
infrastructure until hosting and the `zaplang-io` account exist.
Crucially, #237 **blocks nothing**: the whole decentralized `git`/`path` flow,
and the registry *read* path (#236) against any static index, work without it.

### 6. Client fetch flow

The registry read path is a new `Registry` trait implementation; MVS, the
lockfile, the cache, and `module_search_paths()` are untouched.

1. **Resolve.**
   For each registry dependency, the client fetches the one index file for its
   normalized name over HTTPS (CDN-cached), parses the version lines, and feeds
   `requirements(name, version)` into the existing MVS resolver.
   Non-registry deps continue through their existing `git`/`path` paths; a graph
   can mix all three sources, and MVS unifies them by `(name, major)` + `cksum`.
2. **Select.**
   MVS picks the max-of-minimums, skipping `yanked` versions for fresh locks
   (§4).
3. **Download + verify.**
   For each selected registry module, download the archive from `dl`, run the
   §2.2 verification, and discard on mismatch.
4. **Cache.**
   Store the verified tree in the global cache (`$ZAP_CACHE`) keyed by
   `(name, version, cksum)`, then materialize into `$ZAP_ZENV/pkg/<name>` for
   the active zenv, exactly as `ops`/`zenv` already materialize `git`/`path`
   modules.
5. **Lock.**
   Write/confirm the `zap.lock` row (`source = "registry+…"`, `checksum =
   "zap1:…"`).

#### 6.1 Offline and vendoring

- A fully-populated global cache + `zap.lock` build **offline**: resolution uses
  the locked set and cached trees, no index fetch.
  An index fetch happens only when the lock must change (add/upgrade).
- `path` sources are already fully offline (D-0052) and are the recommended
  air-gap story today.
- A future `zap vendor` (Open issues) copies all locked modules into an in-repo
  `vendor/` tree for fully network-free, registry-free builds — a natural
  extension of the existing materialize step, not new resolution logic.

#### 6.2 How it slots into the existing manager

The only front-end change is the new `Source::Registry` variant and its lock
string; everything downstream (MVS, checksum verification, cache layout,
`module_search_paths()`) is reused.
The `Registry` trait already abstracts "given a name+version, what are its
requirements" — the registry client is simply the first *remote*
implementation of it, where today's tests use an in-memory one.

### 7. Sequencing — #236 and #237

Confirming and refining the epic breakdown:

- **#235 (this doc).**
  Design only: index model, checksum scheme, naming, versioning/yank, publish +
  auth sketch, client fetch flow, and the org-move hosting dependency.
  Stop for review; implement nothing.

- **#236 — index + checksum client (the read path).**
  Implements, all testable with **no hosting** (a local static-file fixture
  tree / `file://` base, or a throwaway `python -m http.server`):
  - the index entry schema + `config.json` parser (§1);
  - the deterministic archive builder + archive-bytes digest, and reuse of the
    existing `zap1:` tree hash as the pinned identity (§2);
  - a `Registry` trait implementation that reads a static index and drives MVS
    (§6 steps 1–2);
  - archive download + full §2.2 verification + cache/materialize (§6 steps
    3–5);
  - the `Source::Registry` manifest/lock support (§3.1) and the
    checksum-mismatch / yanked-selection / `min_zap` diagnostics with `ZAPnnnn`
    codes (D-0071).
  #236 needs only an HTTPS/`file://` fetcher; it does **not** need the real
  registry to exist, so it can land before the org move.

- **#237 — `zap publish` (the write path).**
  Implements the §5 flow: packaging, `zap login`/token store, the authenticated
  `api` client, and ownership/yank operations.
  **Gated on the org move** for real infrastructure (write API, token store,
  archive bucket, the `zaplang.org` host); until then it can be built and tested
  against a local stub `api`, but not deployed.
  It is the **last** registry ticket and blocks nothing (§5.2).

## Rationale

- **Static index over git/dynamic (§1.4).**
  It is the cheapest thing that scales, the easiest to mirror and move, and the
  only option that resolves one name without downloading the world — the right
  fit for infrastructure that does not yet exist.
- **Tree hash as the one identity (§2).**
  Reusing `zap1:` makes a lockfile pin source-agnostic: the registry is "another
  way to fetch the same content," not "a different trust root."
  It also means #236 reuses `checksum.rs` verbatim and adds only transport.
- **Optional, additive registry (D-0052).**
  D-0052 locked "no mandatory central registry"; this honors it by making the
  registry a fourth source and a shared *view* of checksums, never a required
  chokepoint.
  `git`/`path` remain first-class.
- **MVS + minimum-version requirements unchanged (D-0052).**
  The registry deliberately does **not** add range operators or a SAT solver;
  determinism and reproducibility are the point.
- **Immutability + yank, not delete (D-0070 §1.4, §3).**
  The package format is a stable surface with a Go-1 promise; a published
  `(name, version, cksum)` must resolve identically forever, so yank (hide) is
  the right knob and delete is reserved for legal/security takedown with an
  explicit tombstone.
- **First-publisher ownership + scopes (§3, §5).**
  The minimal model that works pre-launch, and scopes are the clean hook for the
  org (`@zaplang/…`, `@acme/…`) once the move happens.
- **Experimental tier (D-0070 §4).**
  Shipping Experimental lets the index/line schema and naming rules be exercised
  and adjusted before the #206 spec-freeze audit folds the stable subset into
  the 1.0 package-format promise.

## Compatibility

- **Additive.**
  No change to existing `zap.toml` / `zap.lock` semantics; `git` and `path`
  projects resolve and build identically.
  The new `registry` source key and the new index-line fields are additive and
  ignorable per D-0070 §1.4, so they are MINOR-grade format changes once 1.0 is
  reached, and free to evolve pre-1.0.
- **Lockfile.**
  `checksum` reuses the `zap1:` value, so a registry row is indistinguishable in
  trust from a `git`/`path` row; only the `source` string is new
  (`registry+…`).
- **Stability tier.**
  Experimental until the #206 audit; the index-line schema, `config.json`, and
  naming rules carry no promise until then.
- **Diagnostics.**
  New conditions (checksum mismatch, yanked selection, name conflict,
  `min_zap` too high) get stable `ZAPnnnn` codes (D-0071); message prose is not
  promised.
- **No IR / runtime-ABI boundary is touched** (host-side `crates/pkg` and the
  CLI only).

## Implementation

Maps onto the epic as §7.
Touches only the host-side package manager and CLI:

1. **#236:** extend `crates/pkg` with an index module (schema + `config.json`),
   a deterministic archiver, a remote `Registry` impl + HTTPS/`file://` fetcher,
   and the `Source::Registry` manifest/lock plumbing; reuse `checksum.rs` and
   the MVS resolver unchanged.
   Diagnostics via `zapc_diag` (D-0071).
   Host-CI only; fully testable against static fixtures, no network, no hosting.
2. **#237:** add `zap publish` / `zap login` to `crates/driver` CLI + a `publish`
   client in `crates/pkg`, against the §5 API contract.
   Gated on the org move for deployment; testable against a local stub.

No phase boundary (IR schema, runtime ABI) is touched, and the work proceeds in
parallel with the rest of the tooling epic.

## Open issues — hosting (deployment decision, gated on org move)

These are **not** decided here; they are named so #236/#237 build against a
clear target and the org move is a config change, not a redesign.

- **Where the index lives.**
  Design target: a static tree under a `zaplang.org` host — e.g.
  `https://index.zaplang.org/` (or `pkg.zaplang.org`) in object storage (S3 /
  R2 / GCS) behind a CDN.
  The client reads the base URL from a built-in default plus a `[registries]`
  table / `ZAP_REGISTRY` override, so re-pointing it after the move is a config
  change.
- **Where archives live.**
  Object storage behind a CDN, addressed by the `config.json` `dl` template;
  independent of the index host.
- **Account / write API host.**
  The authenticated `api` endpoint and token store for #237 — real infra that
  requires the `zaplang-io` org/account.
- **Default registry identity before vs after the move.**
  Today's code lives at `parveshgarg/zap`; the default index/base must be a
  value that can be redirected to the `zaplang-io`/`zaplang.org` endpoints
  without a format change.
  Treat the baked-in default as provisional until the move.
- **Alternate / corporate indexes (§3.1 `index`).**
  A `[registries]` table in `zap.toml` maps an index id to a base URL, so
  private/corporate static indexes work with the same client — no code change,
  just configuration.
- **Future hardening (not required for #236/#237).**
  A signed, append-only `zap.sum` transparency log and mirror cross-checking; a
  `zap vendor` in-repo vendoring command; a dormant-name reclamation policy;
  hash agility to a `zap2:` scheme.

## Addendum (I#237): the implemented publish/discovery client — D-0083 (Proposed)

> **Status: Proposed** (implemented and tested; not yet accepted). This addendum
> records what I#237 actually builds on top of the §5 spec and names the one
> decision it concretizes, so the maintainer can accept or amend it. The
> hosting/auth infrastructure it depends on (§5.2, §7, Open issues) is still a
> deployment decision and is **not** decided here.

**Decision.** `zap publish` targets a **writable static index on a local path or
`file://` URL** as its concrete, implementable realization of the §5 flow, and
the hosted authenticated `api` write path (§5 step 3–4) stays **gated** until
the infrastructure exists.

Rationale: §1 already makes the *read* side a tree of static files, and §8.3 /
Open issues already anticipate "private/corporate static indexes work with the
same client — just configuration." Publishing to such an index is therefore not
a new trust model, it is the inverse of the read path: build the deterministic
archive (§2.1), compute the `zap1:` tree hash + archive digest (§2.2), append
one index line (§1.2), and write `config.json` on first use. This is exactly
what a CI job staging to object storage, or an operator maintaining an internal
mirror, does — and it needs no server, so it is fully testable offline and
deployable today. Inventing a bespoke hosted service to satisfy #237 is what the
design explicitly tells us *not* to do before the org move (§5.2).

**What landed (host-side `crates/pkg` + CLI only; no phase boundary touched):**

- `crates/pkg/src/publish.rs` — validate `[package]`, package the tree, and
  publish to a local static index: create `config.json` (default `dl` =
  `<index>/archives/{name}/{name}-{vers}.zap.tar`, the current `Store` codec) on
  first use, write the archive, and **append** the version's index line.
  Immutability (§4) is enforced: re-publishing an existing `(name, version)` is
  refused. Also the out-of-repo credential store for `zap login`
  (`$ZAP_CACHE/credentials`), per §5 step 3.
- `crates/pkg/src/discovery.rs` — `zap search` enumerates a local/`file://`
  index tree and matches on the normalized name; `zap info` is a direct
  name → one-file lookup that works against **any** static index (local or
  remote), reusing the §6 read client.
- CLI verbs `zap publish [--index|--registry] [--dry-run]`,
  `zap login [--index|--registry] [--token]`, `zap search <query>`,
  `zap info <name>` (D-0053 surface; additive/Experimental under D-0076).

**The hosting gap this surfaces (unchanged from §5.2 / §7 / Open issues).** The
authenticated HTTP write API, the token-issuing account service, and the archive
bucket are real hosted infrastructure that does not exist and is gated on the
org move. The client is honest about it rather than faking it:

- Publishing to an `https://` index returns `ZAP6108` ("registry publish target
  unavailable"), naming the gap and pointing at a local/`file://` index.
- `zap login` writes a real local token store, but the token is only *consumed*
  by that gated remote path; a local index needs none. This is stated at the
  point of use.
- `zap search` over a remote static index returns a clear error (no directory
  listing exists to enumerate; §1.4 rejected a dynamic search service), pointing
  at `zap info` for a direct lookup. A hosted search endpoint (served from
  `config.json`'s `api`, or a generated catalog file) is a follow-up on the same
  hosting decision.

**New diagnostics (D-0071; back-end category `6`, Experimental):** `ZAP6106`
(version already published), `ZAP6107` (incomplete package for publish),
`ZAP6108` (publish target unavailable / hosted write API gated), `ZAP6109`
(missing or invalid registry credentials).

**Open for the maintainer (the deployment decision, not this ticket):** stand up
the write `api` + token/account service + archive bucket under the `zaplang.org`
host; decide the default index base baked into the client (provisional until the
move, Open issues); and whether remote discovery gets a search endpoint or a
generated catalog. When those land, the client re-points via `config.json` /
`[registries]` / `ZAP_REGISTRY` with no format change, and the `https://` publish
path can be ungated behind the token store that already exists.
