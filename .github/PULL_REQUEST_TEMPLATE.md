<!-- Reference issues as I#n and PRs as PR#n. Link with "Closes #n" below. -->

Closes #

## What and why

<!-- One or two paragraphs. Link the design doc / D-NNNN if there is one. -->

## Checklist

**CI gates** (run locally; see CONTRIBUTING.md)
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo clippy --workspace --features zapc/gc-immix --all-targets -- -D warnings`
- [ ] `cargo clippy --workspace --features zapc/gc-boehm --all-targets -- -D warnings`
- [ ] `make docker-test` passes under **both** collectors (Immix and Boehm)
- [ ] CI is green for **both** the `pull_request` **and** `push` triggers (`host` + `full`)

**Tests**
- [ ] New/changed behavior has golden programs in `tests/golden/` (`.zap` + `.expected`)
- [ ] Goldens are deterministic (no exact GC/heap byte counts)

**Language semantics** (delete this block for a non-language PR — tooling, CI, docs, runtime-internal)
- [ ] Walked the [Python-divergence checklist](../docs/DIVERGENCE_CHECKLIST.md) for the behavior this PR touches
- [ ] Edge cases are pinned as conformance cases in `tests/conformance/` and their `CLAIMS.md` rows flipped to `covered` (epic #374 / I#376)
- [ ] Every intentional divergence from Python has a `D-00xx` line **and** a case that pins it (`tests/conformance/` + `tests/diff/` `# diff: diverge`)
- [ ] Any divergence found but not fixed here is filed and left as a `CLAIMS.md` `TODO (impl gap …)`, not pinned as passing

**Docs**
- [ ] Design doc in `design/` added/updated, and a `DECISIONS.md` entry (`D-NNNN`) for new decisions
- [ ] `SPEC.md` updated if language behavior changed
- [ ] `docs/GRAMMAR.ebnf` updated if the grammar changed
- [ ] Uses "coroutine", not "goroutine" (except when referring to Go)

<!-- Delete any item that genuinely does not apply, and say why in the description. -->
