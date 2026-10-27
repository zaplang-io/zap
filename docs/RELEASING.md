# Releasing Zap

**Status:** Process (I#207, epic #158 / `design/0016-platform-1.0.md` Area E1).
**Last updated:** 2026-10-27

This is the **release process**: the git-tag convention, how the workspace
`version` bumps, and how `CHANGELOG.md` rolls from `[Unreleased]` into a
versioned section when a release is cut.

It does **not** decide the versioning rules — those are fixed by
[`docs/STABILITY.md`](STABILITY.md), which is the **source of truth** for what a
bump means. This doc is the mechanical companion to that policy. Building
prebuilt binaries and installers is a **separate** track (I#208, Area E2) and is
out of scope here.

---

## 1. One version, SemVer 2.0.0

Zap ships as **one artifact** carrying **one version** — the
`[workspace.package] version` in the root `Cargo.toml`. That version is
[Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).

A single version covers several surfaces (Language, standard library, CLI,
package format, runtime/ABI). **The release's bump is the maximum change class
over all surfaces** since the previous release (STABILITY §1): a breaking change
to any one Stable surface forces a MAJOR; a backward-compatible addition
anywhere forces at least a MINOR; a bug fix with no surface change is a PATCH.

- **Experimental and Internal surfaces do not gate the bump.** They may change
  in any release with only a changelog note (STABILITY §4.3), so a change
  confined to them is at most a MINOR (the addition) or a PATCH. Only **Stable**
  surfaces can force a MAJOR.
- **What is Stable** is the D-0076 ratified set (STABILITY §4.4). Consult it, not
  intuition, when classifying a change.

Read STABILITY §1 (`MAJOR`/`MINOR`/`PATCH` per surface), §3 (the 1.0 promise and
its bug-fix / security carve-outs), and §5–§6 (deprecation and the breaking-change
process) before deciding a version. Do not restate or re-derive those rules here.

### 1.1 We are pre-1.0 (the `0.x` regime)

The current version is `0.0.1` and **no release has been tagged yet**. Under
SemVer, `0.x` makes **no** compatibility promise (STABILITY §2). Concretely, for
now:

- A `0.MINOR` bump **may** contain breaking changes to any surface, including the
  language. A `0.x.PATCH` bump is **bug-fixes only** (the §1.3 no-surface-change
  rule).
- The `SPEC.md` `Version:` header (currently `0.3`) is an **editorial** revision
  of the spec document and moves **independently** of the toolchain version in
  `Cargo.toml`. The `Cargo.toml` version is the **authoritative release version**.
- The `1.0.0` promise and the frozen Stable set (D-0076) are **deferred to the
  actual 1.0 release**. Cutting a `0.x` release does **not** stamp 1.0 and does
  **not** turn on the compatibility promise.

### 1.2 Pre-release tags on the road to 1.0

Pre-releases use SemVer pre-release identifiers and sort before the final:
`1.0.0-alpha.N` → `1.0.0-beta.N` → `1.0.0-rc.N` → `1.0.0` (STABILITY §2). A
pre-release makes no stability promise; it exists to exercise the Stable set
before it is frozen.

---

## 2. The changelog contract

[`CHANGELOG.md`](../CHANGELOG.md) at the repo root follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/): a top `[Unreleased]`
section with `Added` / `Changed` / `Deprecated` / `Removed` / `Fixed` /
`Security` headings, then one dated section per released version, newest first.

- **Every PR that changes a public surface adds a `[Unreleased]` entry** under
  the right heading. This is a release-process gate (STABILITY §9); a
  surface-changing PR without a changelog entry is incomplete.
- State the change class (PATCH / MINOR / MAJOR) for a surface change in the PR
  description, by the STABILITY §1 rules, so the eventual bump is easy to total.
- Entries describe **user-visible** change (what a `.zap` author, CLI user, or
  tool integrator sees), not internal refactors. Refer to issues/PRs as `I#n` /
  `PR#n` and to decisions as `D-NNNN`, per `CONTRIBUTING.md`.
- Behaviour changes from the bug-fix (§3.2) or security/soundness (§3.3)
  carve-outs that can affect real code go under `Fixed` / `Security` and are
  called out prominently.

---

## 3. Cutting a release

Releases are cut by the **maintainer**. The irreversible, outward-facing steps
(the tag and any publish) are theirs; the steps below prepare the commit that the
tag will point at.

1. **Pick the version.** Total the change class of everything under
   `[Unreleased]` since the last release and take the **maximum** (§1). In `0.x`,
   any breaking surface change makes it a `0.MINOR` bump; a bug-fix-only set is a
   `0.x.PATCH`. For anything not yet frozen on the road to 1.0, use a pre-release
   tag (§1.2).
2. **Roll the changelog.** Rename `[Unreleased]` to `[X.Y.Z] - YYYY-MM-DD`,
   prune any now-empty headings, and add a fresh empty `[Unreleased]` block above
   it. Update the link references at the bottom (`[Unreleased]` compares the new
   tag to `HEAD`; add `[X.Y.Z]` comparing it to the previous tag).
3. **Bump `Cargo.toml`.** Set `[workspace.package] version = "X.Y.Z"` and run a
   build so `Cargo.lock` updates. The helper in §5 does steps 2–3.
4. **(1.0 only.)** Reconcile `SPEC.md`'s editorial `Version:` header to `1.0` —
   from 1.0 on it tracks the language MAJOR (STABILITY §2). Not done for `0.x`
   releases.
5. **Commit** on a branch: `docs(release): vX.Y.Z` (or `chore(release): vX.Y.Z`),
   open a PR, and get **maintainer sign-off**. Release PRs stop for review like
   any process change.
6. **Tag and push** (maintainer). After the release commit is on `main`, create
   an **annotated** tag on it and push the tag:

   ```bash
   git tag -a vX.Y.Z -m "Zap vX.Y.Z"
   git push origin vX.Y.Z
   ```

### 3.1 Tag convention

- Final releases: **`vX.Y.Z`** (leading `v`), annotated, on the release commit.
- Pre-releases: `v1.0.0-alpha.N`, `v1.0.0-beta.N`, `v1.0.0-rc.N` (§1.2).
- Tags are **immutable** — never move or delete a published tag. A mistake is
  fixed by cutting the next version.

---

## 4. The 1.0 cut

`1.0.0` is cut when the **maintainer declares the Stable surface frozen for 1.0**
— the D-0076 spec-freeze set (STABILITY §4.4) is what gets frozen. At that point:

- The §3 compatibility promise takes effect for the entire `1.x` series.
- `SPEC.md` is stamped `1.0` and thereafter tracks the language MAJOR.
- Subsequent changes follow the full post-1.0 regime: additive at MINOR (§1.2),
  deprecation window (§5) before any Stable removal, and the breaking-change
  process (§6) for a MAJOR.

Until the maintainer makes that call, the project stays editorial `0.x` (§1.1).

### 4.1 Support windows (post-1.0)

The durations are policy (STABILITY §7.2): only the latest MINOR of the current
MAJOR gets routine PATCH releases, and a superseded MAJOR gets security /
critical-correctness fixes for **at least 12 months**. The mechanics for
delivering those (release branches, backports, cadence) are owned by this track
and will be fleshed out when the first such need arises; they are not needed
pre-1.0.

---

## 5. Helper script (optional)

[`scripts/release.sh`](../scripts/release.sh) automates the mechanical,
reversible edits only — rolling `CHANGELOG.md`'s `[Unreleased]` into a dated
`[X.Y.Z]` section and bumping the `Cargo.toml` version. It **does not** commit,
tag, or push: those stay with the maintainer (§3). Everything it does can be done
by hand with the steps above.

```bash
scripts/release.sh 0.1.0          # roll changelog + bump Cargo.toml to 0.1.0
scripts/release.sh --help
```

After it runs, review the diff, build to refresh `Cargo.lock`, then follow §3
from the commit step.

---

## 6. Out of scope

- **Prebuilt binaries + installers** (per-platform artifacts, shell installer,
  Homebrew / Scoop): **I#208 / Area E2.** It depends on the platform work it
  ships and on this process. macOS artifacts are additionally hardware-gated
  (no CI runner yet) and parked.
- **Release CI / publish automation:** not set up here. Releases are cut by hand
  per §3 until the artifact track (I#208) lands.
- **Changing the versioning or stability rules:** those live in
  [`docs/STABILITY.md`](STABILITY.md) and change only through the design-doc +
  `DECISIONS.md` process.
