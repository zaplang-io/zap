# Decisions (ADR files)

This directory is the **going-forward decision log**. Each locked decision is a
**single file**, so two concurrent pull requests recording different decisions
never touch the same file and never conflict.

It replaces the append-to-tail workflow of the historical
[`DECISIONS.md`](../DECISIONS.md), which stays as the frozen record of
`D-0001…D-0243`. See that file's header for the cutover note. In-flight PRs that
already added a `D-####` entry to `DECISIONS.md` are grandfathered; **new**
decisions go here.

## Why per-file, and why no more `D-####`

Two root causes made the old single file a bottleneck as contributor count grew:

1. **Tail-append conflicts** — nearly every PR appended an entry to the end of
   `DECISIONS.md`, so nearly every pair of concurrent PRs conflicted there.
2. **Hand-allocated sequential IDs** — two concurrent PRs both grabbed the same
   "next" `D-####` and collided, a conflict no merge tool can resolve.

The fix removes both: one file per decision (distinct filenames, no shared tail),
and **the decision's identity is its GitHub issue/PR number**, which is globally
unique in GitHub's shared issue/PR number space — so two decisions can never be
assigned the same key, even when authored at the same time.

## Layout

Decisions are sharded by domain:

```
decisions/
  lang/      language surface, type system, grammar, semantics
  stdlib/    standard library modules and their surface
  runtime/   GC, scheduler, codegen/ABI, runtime C
  tooling/   CLI, fmt, LSP, CI, packaging, repo process
  README.md  (this file)
  TEMPLATE.md
```

(There is no committed `INDEX.md`: the index is a derived view generated on
demand — see [The index](#the-index) below.)

Pick the single domain that best fits; if a decision spans two, file it under the
one that owns the surface it changes and cross-link the other in the body.

## Adding a decision

1. Copy [`TEMPLATE.md`](TEMPLATE.md) to
   `decisions/<domain>/<number>-<slug>.md`, where `<number>` is the **governing
   issue number** if the decision closes one (issues are filed first, so the
   number is stable earliest), otherwise the **PR number**. `<slug>` is a short
   kebab-case summary.
2. Fill in the frontmatter (see below) and write the decision and its
   **rationale** — the same substance an old `D-####` entry carried (Status, the
   decision, why, alternatives rejected).
3. There is no index file to touch — it is generated on demand (see below). CI
   only validates that your ADR carries the required frontmatter.
4. Link related decisions by path, e.g. `[runtime/869-...](../runtime/869-...md)`.

### Frontmatter

```yaml
---
title: One-line decision title
status: Proposed | Locked | Superseded
issue: 1089          # governing issue, if any
pr: 1092             # introducing PR, if known
date: 2026-11-19
domain: tooling      # lang | stdlib | runtime | tooling
supersedes:          # optional: number(s) of decisions this overrides
superseded-by:       # optional: filled in when a later decision overrides this
---
```

`status`, `title`, `date`, `domain` and at least one of `issue`/`pr` are
required; the generator keys the entry on `issue` when present, else `pr`.

### Superseding a decision

Never delete a decision. When a later decision overrides an earlier one, set the
earlier file's `status: Superseded` and its `superseded-by:` to the new number,
and list the old number in the new file's `supersedes:`. Both files stay.

## The index

The per-file ADRs in this tree are the **source of truth**; the index is a
**derived view** and is deliberately **not committed** to git
(zaplang-io/zap#1321). A single generated aggregate that every ADR-adding PR had
to regenerate and commit was a standing merge-conflict hotspot — the same
pathology the frozen [`DECISIONS.md`](../DECISIONS.md) had, which is exactly why
we moved to per-file ADRs. So there is nothing to keep in sync by hand.

[`scripts/gen-decisions-index.sh`](../scripts/gen-decisions-index.sh) produces the
index on demand (grouped by domain, sorted by number); the docs site renders the
same view from the ADR frontmatter. CI does **not** check a committed copy — it
validates the ADR *sources* (every ADR parses and carries the required
frontmatter):

```bash
scripts/gen-decisions-index.sh            # write decisions/INDEX.md locally (gitignored) for browsing
scripts/gen-decisions-index.sh -o -       # print the index to stdout
scripts/gen-decisions-index.sh --validate # what CI runs: validate the ADR frontmatter
```
