# Changelog fragments

A pull request that changes a public surface adds a **fragment file here**
instead of editing the tail of [`CHANGELOG.md`](../CHANGELOG.md). Fragments are
assembled into the `[Unreleased]` section when a release is cut
([`docs/RELEASING.md`](../docs/RELEASING.md)).

This removes the contention that made `CHANGELOG.md` a bottleneck: every PR used
to append to the same `[Unreleased]` block, so concurrent PRs conflicted there.
Distinct fragment files never conflict.

## Writing a fragment

Create `changelog.d/<number>.<kind>.md`:

- `<number>` — the PR number (or the governing issue number) the entry belongs
  to. GitHub's issue/PR number space is globally unique, so fragment filenames
  never collide across concurrent PRs.
- `<kind>` — one of the Keep-a-Changelog headings, lowercased:
  `added`, `changed`, `deprecated`, `removed`, `fixed`, `security`.

A PR that spans two kinds adds two files (e.g. `1092.added.md` and
`1092.fixed.md`).

The file content is the Markdown bullet(s) exactly as they should appear under
that heading — the same user-visible wording the old inline entry carried, with
`I#n` / `PR#n` / `D-NNNN` references per `CONTRIBUTING.md`. Example
`changelog.d/1092.added.md`:

```markdown
- **`foo.bar` — one-line summary** (PR#1092, decisions/stdlib/1090-...). What a
  `.zap` author sees, and any intentional divergence from Python.
```

Only surface-changing PRs need a fragment; a pure refactor, a CI/process change,
or an internal-only change does not (same rule as the old inline changelog).

## Assembling

[`scripts/build-changelog.sh`](../scripts/build-changelog.sh) collects the
fragments, groups them by kind in Keep-a-Changelog order, and prints the block:

```bash
scripts/build-changelog.sh            # print the assembled [Unreleased] additions
scripts/build-changelog.sh --check    # validate fragment filenames (CI gate)
```

At release time the assembled block is folded into `CHANGELOG.md`'s
`[Unreleased]` section and the consumed fragments are deleted (see
`docs/RELEASING.md`). Existing `[Unreleased]` content is preserved — fragments
and any pre-cutover inline entries are merged.
