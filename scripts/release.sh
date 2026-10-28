#!/usr/bin/env bash
#
# release.sh — prepare a Zap release (I#207). OPTIONAL helper.
#
# Automates only the mechanical, reversible edits described in
# docs/RELEASING.md §3: roll CHANGELOG.md's [Unreleased] section into a dated
# [X.Y.Z] section (leaving a fresh empty [Unreleased] above it) and bump the
# [workspace.package] version in the root Cargo.toml.
#
# It deliberately does NOT commit, tag, or push — those irreversible,
# outward-facing steps stay with the maintainer (docs/RELEASING.md §3). Review
# the diff, rebuild to refresh Cargo.lock, then follow RELEASING.md from the
# commit step.
#
# Usage:
#   scripts/release.sh X.Y.Z[-pre.N]
#   scripts/release.sh --help
#
set -euo pipefail

REPO_URL="https://github.com/zaplang-io/zap"

usage() {
  sed -n '3,17p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

case "${1:-}" in
  -h|--help|"") usage 0 ;;
esac

VERSION="$1"
# SemVer 2.0.0: MAJOR.MINOR.PATCH with an optional pre-release (e.g. 1.0.0-rc.1).
if ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
  echo "error: '$VERSION' is not a SemVer version (expected X.Y.Z or X.Y.Z-pre.N)" >&2
  exit 2
fi

# Run from the repo root regardless of the caller's cwd.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

CHANGELOG="CHANGELOG.md"
CARGO="Cargo.toml"
DATE="$(date +%Y-%m-%d)"

[ -f "$CHANGELOG" ] || { echo "error: $CHANGELOG not found" >&2; exit 1; }
[ -f "$CARGO" ]     || { echo "error: $CARGO not found" >&2; exit 1; }

if ! grep -q '^## \[Unreleased\]' "$CHANGELOG"; then
  echo "error: no '## [Unreleased]' section in $CHANGELOG" >&2
  exit 1
fi
if grep -q "^## \[$VERSION\]" "$CHANGELOG"; then
  echo "error: $CHANGELOG already has a [$VERSION] section" >&2
  exit 1
fi

# 1. Roll the changelog: insert a fresh empty [Unreleased] block, date the old
#    one as [X.Y.Z], and rewrite the link references at the bottom.
awk -v ver="$VERSION" -v date="$DATE" -v url="$REPO_URL" '
  !rolled && /^## \[Unreleased\]/ {
    print "## [Unreleased]"; print "";
    print "### Added"; print "";
    print "### Changed"; print "";
    print "### Deprecated"; print "";
    print "### Removed"; print "";
    print "### Fixed"; print "";
    print "### Security"; print "";
    print "## [" ver "] - " date;
    rolled = 1;
    next
  }
  /^\[Unreleased\]:/ {
    print "[Unreleased]: " url "/compare/v" ver "...HEAD";
    print "[" ver "]: " url "/releases/tag/v" ver;
    next
  }
  { print }
' "$CHANGELOG" > "$CHANGELOG.tmp"
mv "$CHANGELOG.tmp" "$CHANGELOG"

# 2. Bump the workspace version. Target the single `version = "..."` line that
#    sits under [workspace.package]; leave every other `version =` alone.
awk -v ver="$VERSION" '
  /^\[workspace\.package\]/ { inpkg = 1 }
  inpkg && !done && /^version[[:space:]]*=/ {
    print "version = \"" ver "\"";
    done = 1; next
  }
  /^\[/ && !/^\[workspace\.package\]/ { inpkg = 0 }
  { print }
' "$CARGO" > "$CARGO.tmp"
mv "$CARGO.tmp" "$CARGO"

echo "Prepared release v$VERSION:"
echo "  - $CHANGELOG: [Unreleased] rolled into [$VERSION] - $DATE"
echo "  - $CARGO: [workspace.package] version = \"$VERSION\""
echo
echo "Next (see docs/RELEASING.md §3):"
echo "  1. Review the diff; trim placeholder lines from the dated section."
echo "  2. Rebuild so Cargo.lock updates."
echo "  3. Commit, open a release PR, get maintainer sign-off."
echo "  4. Maintainer tags: git tag -a v$VERSION -m 'Zap v$VERSION' && git push origin v$VERSION"
