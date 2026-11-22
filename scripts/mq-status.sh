#!/usr/bin/env bash
# mq-status.sh — "why did PR #N fall out of the merge queue?"
#
# Read-only companion to the `merge-queue-report` CI job (I#1268). The full
# LLVM/aarch64/Windows matrix runs only on the `merge_group` candidate, so its
# results attach to the ephemeral gh-readonly-queue/<base>/pr-<N>-<sha> ref, not
# the PR head. The CI job posts the FIRST failure back to the PR automatically;
# this script is the on-demand terminal view of a PR's most recent queue run.
#
# The queue *branch* is ephemeral and usually already deleted by the time you
# look, but the Actions *run* and its per-job conclusions persist, so this still
# works after the eviction.
#
# Usage:
#   scripts/mq-status.sh <pr-number>
#
# Requires: gh (GitHub CLI, authenticated) and jq. No secrets, no writes — it
# only reads the Actions API for the current repository.
set -euo pipefail

die() {
  echo "error: $*" >&2
  exit 2
}

pr="${1:-}"
case "$pr" in
  '' | *[!0-9]*) die "usage: $0 <pr-number>" ;;
esac

command -v gh >/dev/null 2>&1 || die "gh (GitHub CLI) is required and not on PATH"
command -v jq >/dev/null 2>&1 || die "jq is required and not on PATH"

repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)" ||
  die "could not determine the repository (run inside a gh-enabled checkout)"

echo "Looking for the most recent merge-queue run for ${repo}#${pr} ..."

# All `merge_group` runs; `.head_branch` is the queue ref name. Keep only this
# PR's candidate(s) — gh-readonly-queue/<base>/pr-<N>-<sha> — and take the newest.
# The `pr-<N>-<hex>$` anchor means a base branch containing slashes cannot be
# mistaken for the PR number.
run_json="$(
  gh api --paginate \
    "repos/${repo}/actions/runs?event=merge_group&per_page=100" \
    --jq '.workflow_runs[]' |
    jq -s --arg pr "$pr" '
      [ .[] | select(.head_branch | test("/pr-" + $pr + "-[0-9a-f]+$")) ]
      | sort_by(.run_started_at) | reverse | .[0] // empty
    '
)"

if [ -z "$run_json" ]; then
  echo "No merge-queue run found for #${pr}."
  echo "It was never queued, or its queue run has aged out of the Actions API."
  echo "(A PR evicted for a NON-failure reason — the base moved, or a PR ahead of"
  echo " it failed — also leaves no failing run of its own.)"
  exit 0
fi

run_id="$(printf '%s' "$run_json" | jq -r '.id')"
run_url="$(printf '%s' "$run_json" | jq -r '.html_url')"
run_concl="$(printf '%s' "$run_json" | jq -r '.conclusion // .status')"
run_branch="$(printf '%s' "$run_json" | jq -r '.head_branch')"

echo
echo "Run:    ${run_url}"
echo "Ref:    ${run_branch}"
echo "Result: ${run_concl}"
echo
echo "Per-job conclusions:"

# Each job's conclusion. Map the raw Actions values onto short labels; anything
# unexpected is printed verbatim so nothing is silently hidden.
gh api --paginate "repos/${repo}/actions/runs/${run_id}/jobs?per_page=100" \
  --jq '.jobs[] | [.name, (.conclusion // .status)] | @tsv' |
  while IFS="$(printf '\t')" read -r name concl; do
    case "$concl" in
      success) label="pass" ;;
      failure) label="FAIL" ;;
      cancelled) label="cancelled" ;;
      timed_out) label="TIMED-OUT" ;;
      skipped) label="skipped" ;;
      *) label="$concl" ;;
    esac
    printf '  %-10s %s\n' "$label" "$name"
  done
