---
title: Surface merge-queue failures on the PR via a merge_group failure-reporter job
status: Accepted
issue: 1268
pr:
date: 2026-11-22
domain: tooling
supersedes:
superseded-by:
---

# Surface merge-queue failures on the PR via a merge_group failure-reporter job

**Status:** Accepted

## Decision

Add one job to `.github/workflows/ci.yml`, `merge-queue-report`, that runs on the
`merge_group` event, `needs:` every job that runs in the queue and can fail (the
`full` LLVM matrix, `arm64`, the four required Windows jobs, plus `changes` and
`host`), and fires only when one of them failed or was cancelled:

```yaml
if: ${{ always() && github.event_name == 'merge_group' && (failure() || cancelled()) }}
```

On firing it posts a **sticky** comment — keyed by a hidden `<!-- mq-report -->`
marker, so repeated evictions update one comment instead of spamming — to the PR
parsed out of the queue ref (`gh-readonly-queue/<base>/pr-<N>-<sha>` → `<N>`). The
comment names the failed/cancelled job(s) and links the run. It carries job-level
`permissions: { pull-requests: write }` only, and is **not** in `ci-gate`'s
`needs:`, so its own outcome cannot block or alter the merge.

The resulting signal is deliberate:

- **Comment present** → the PR genuinely failed (or was cancelled/timed-out) in
  the queue; the linked run has the logs.
- **No comment, yet the PR fell out** → a non-failure eviction: the base moved,
  or a PR *ahead* of it failed. Nothing this job `needs:` failed, so it never
  ran.

A read-only companion, `scripts/mq-status.sh <pr>`, prints the per-job
conclusions of a PR's most recent queue run on demand from the terminal (it reads
the persisted Actions run, since the queue branch itself is already gone).

## Rationale

The CI gates are split on purpose (Epic #1088 / Discussion #1087): the fast host
build runs on `pull_request`, and the full LLVM 18 / aarch64 / Windows matrix
runs only on the `merge_group` candidate. That split exists to stop the matrix
from burning GitHub Actions minutes on every PR push while keeping "main always
builds, tested-as-rebased".

The cost of the split is legibility. Because the matrix runs only in the queue,
its check results attach to the ephemeral `gh-readonly-queue/<base>/pr-<N>-<sha>`
ref, never the PR head, and GitHub does not mirror them to the PR. An eviction
leaves only a terse timeline event, and the failing logs vanish with the queue
branch — so from the PR you cannot tell *failed in the queue* from *never queued*
from *evicted because a PR ahead failed*. With several PRs cycling through the
queue this is hard to reason about (e.g. #1263 was evicted on a 60s gc-boehm
golden-timeout flake with nothing on the PR to say so).

Alternatives rejected:

- **Re-add the full matrix to `pull_request`.** This would surface failures on
  the PR directly, but it re-introduces exactly the per-push matrix cost the
  fast/full split was created to eliminate — the maintainer's local Docker/LLVM
  is down and CI is the *sole* gate, so minutes are scarce. A reporter job adds
  the visibility at the cost of ~one cheap ubuntu step, and only when a queue run
  genuinely fails. This is the whole point of the change: keep the split, add
  visibility.
- **A separate `workflow_run`-triggered workflow.** More moving parts (a second
  workflow, cross-workflow run correlation) for no gain over a job in the same
  run that already has every matrix result in its `needs:` context.
- **Widen the workflow-default permissions.** The reporter needs only
  `pull-requests: write`; scoping it at the job keeps every other job on the
  read-only default (least privilege).
- **Fresh comment per eviction.** Spams the PR. The hidden-marker sticky update
  keeps exactly one authoritative comment.
- **Put the reporter in `ci-gate`'s `needs:`.** That would let a hiccup in the
  reporter (API blip, rate limit) turn the required gate red and block a merge.
  It is excluded so reporting is strictly additive.

## Consequences

- `merge_group` cannot be triggered locally, so the reporter only truly proves
  out on the next real queued PR. Ahead of that it was validated statically:
  `actionlint` on the workflow (clean for this job), the `pr-<N>` parse
  dry-run against sample refs **and** against a real queue ref observed via
  `scripts/mq-status.sh` (`gh-readonly-queue/main/pr-1266-<sha>`), and
  `shellcheck` on the helper. The PR says end-to-end validation lands on the
  next eviction.
- `cancelled()` coverage names timeout/eviction cases (the known 60s gc-boehm
  golden-timeout flakes) explicitly. It also fires when the merge queue cancels a
  candidate because a PR *ahead* failed; in that cancellation-only case the
  comment says so and tells the reader to check the run before assuming this PR
  is at fault, rather than asserting a genuine failure.
- `windows-crates` is intentionally excluded from `needs:` — it is
  `continue-on-error` (never reports failure to dependents), exactly as it is
  excluded from `ci-gate`.
- No minutes are added to the `pull_request` fast gate, and the full matrix is
  not re-introduced on `pull_request`.

See `.github/workflows/ci.yml` (the `merge-queue-report` job and the fast/full
split header) and `scripts/mq-status.sh`.
