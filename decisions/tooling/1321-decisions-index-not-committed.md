---
title: The decision index is a derived view, generated on demand — not committed to git
status: Accepted
issue: 1321
pr:
date: 2026-11-23
domain: tooling
supersedes:
superseded-by:
---

# The decision index is a derived view, generated on demand — not committed to git

**Status:** Accepted

## Context

The per-file ADR migration (#1089) froze `DECISIONS.md` and moved each decision to
`decisions/<domain>/<number>-slug.md`, eliminating the single-file merge-conflict
hotspot that the append-only `DECISIONS.md` had been. But it also introduced
`decisions/INDEX.md`: a single, checked-in, **generated aggregate** of all ADRs,
kept current by a CI gate (`gen-decisions-index.sh --check`) that failed if the
committed copy was stale.

That re-created the exact problem the migration set out to solve. Every
ADR-adding PR had to regenerate and commit `INDEX.md`; concurrent PRs collided on
it, and every rebase re-ran the generator and re-conflicted. (`pr-size.yml`
already excluded it from the size count — a tacit admission it was noise.) The
changelog, by contrast, had the right shape all along: `changelog.d/<n>.<kind>.md`
fragments are per-file (no contention) and `CHANGELOG.md` is assembled downstream,
not committed per PR. The index simply never got that treatment.

## Decision

Treat the index the way the changelog already treats `CHANGELOG.md`: the per-file
ADRs are the source of truth, and the index is a **derived view** that is **not
committed to git**.

- Remove `decisions/INDEX.md` from version control and `.gitignore` it (a locally
  generated copy is fine for browsing).
- `scripts/gen-decisions-index.sh` still renders the index on demand: default
  writes `decisions/INDEX.md` locally; `-o -` prints to stdout (for the docs site
  / piping). The docs site renders the same view from ADR frontmatter.
- The CI gate (`process-checks.yml`) switches from `--check` (compare a committed
  artifact) to `--validate`: it parses every ADR and fails only if one is missing
  a required frontmatter field (`title`/`status`/`date` and at least one of
  `issue`/`pr`). Duplicate decision numbers are **allowed** — one issue
  legitimately spans several stage ADRs (e.g. #1298), and the index lists each.

Result: there is no shared mutable generated file for PRs to fight over, while the
gate still catches malformed ADRs, and contributors have nothing to regenerate or
keep in sync by hand.

## Alternatives considered

- **Regenerate the committed index post-merge via a bot** (keep it in-repo, drop
  the PR gate). Works, but needs a bot commit per ADR-merge and a branch-protection
  bypass for the bot, and keeps a derived artifact in the tree for no real gain —
  the browsable view is better served by the docs site.
- **Shard the index per domain.** Only narrows contention (same-domain PRs still
  collide) and still needs a top-level aggregate. Partial, not a cure.
- **Keep `--check` but only on `merge_group`.** The rebased queue candidate still
  needs the committed index current, so it does not remove the conflict.

## Consequences

- Revises the index mechanism of #1089 (the per-file ADR model and frozen
  `DECISIONS.md` are unchanged).
- The "regenerate `decisions/INDEX.md` and commit it after adding an ADR" step is
  retired; contributor docs (`decisions/README.md`, `README.md`,
  `docs/REPO_ARCHITECTURE.md`) are updated accordingly.
- Host-safe: the generator stays POSIX sh + awk + sort, no LLVM, no network.
