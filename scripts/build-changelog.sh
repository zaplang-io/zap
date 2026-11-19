#!/bin/sh
# Assemble changelog fragments from changelog.d/ into a Keep-a-Changelog block.
#
# PRs drop a fragment at changelog.d/<number>.<kind>.md instead of editing the
# tail of CHANGELOG.md (see changelog.d/README.md). This script groups the
# fragments by kind, in Keep-a-Changelog order, and prints the resulting
# `### Added` / `### Changed` / ... block to stdout. It does NOT mutate
# CHANGELOG.md — at release time the maintainer folds the printed block into the
# `[Unreleased]` section and deletes the consumed fragments (docs/RELEASING.md).
#
# Usage:
#   scripts/build-changelog.sh            # print the assembled block
#   scripts/build-changelog.sh --check    # validate fragment filenames; exit 1 on any bad name
#
# Host-safe: POSIX sh only, no LLVM, no network.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$script_dir/.." && pwd)
frag_dir="$root/changelog.d"

# Keep-a-Changelog heading order.
kinds="added changed deprecated removed fixed security"

heading_for() {
  case "$1" in
    added) echo "Added" ;; changed) echo "Changed" ;;
    deprecated) echo "Deprecated" ;; removed) echo "Removed" ;;
    fixed) echo "Fixed" ;; security) echo "Security" ;;
    *) echo "" ;;
  esac
}

is_kind() {
  for k in $kinds; do [ "$1" = "$k" ] && return 0; done
  return 1
}

[ -d "$frag_dir" ] || { echo "no changelog.d/ directory" >&2; exit 0; }

# --check: every *.md under changelog.d/ except README.md must be
# <number>.<kind>.md with a valid kind. Report all offenders, then fail.
if [ "${1:-}" = "--check" ]; then
  bad=0
  for f in "$frag_dir"/*.md; do
    [ -e "$f" ] || continue
    base=$(basename "$f")
    [ "$base" = "README.md" ] && continue
    # Expect exactly <number>.<kind>.md
    num=$(printf '%s' "$base" | sed -n 's/^\([0-9][0-9]*\)\.\([a-z][a-z]*\)\.md$/\1/p')
    kind=$(printf '%s' "$base" | sed -n 's/^\([0-9][0-9]*\)\.\([a-z][a-z]*\)\.md$/\2/p')
    if [ -z "$num" ] || [ -z "$kind" ] || ! is_kind "$kind"; then
      echo "ERROR: bad changelog fragment name: changelog.d/$base" >&2
      echo "       expected changelog.d/<number>.<kind>.md, kind in: $kinds" >&2
      bad=1
    fi
  done
  if [ "$bad" -ne 0 ]; then exit 1; fi
  echo "changelog.d/ fragment names OK."
  exit 0
fi

# Default: print the assembled block, grouped by kind.
any=0
for kind in $kinds; do
  # Collect matching fragments, sorted by number.
  files=$(ls "$frag_dir"/*."$kind".md 2>/dev/null | sort -t/ -k2 -n || true)
  [ -z "$files" ] && continue
  any=1
  echo "### $(heading_for "$kind")"
  echo
  printf '%s\n' "$files" | while IFS= read -r f; do
    [ -e "$f" ] || continue
    # Emit the fragment body verbatim, trimming a single trailing blank line.
    sed -e '$ { /^$/d; }' "$f"
  done
  echo
done

[ "$any" -eq 0 ] && echo "(no changelog fragments)" >&2
exit 0
