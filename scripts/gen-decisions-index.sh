#!/bin/sh
# Generate / validate the decision index from the per-file ADRs under decisions/.
#
# One file per decision lives at decisions/<domain>/<number>-<slug>.md with YAML
# frontmatter (see decisions/README.md). The per-file ADRs are the SOURCE OF
# TRUTH; the index is a derived VIEW of them, grouped by domain and sorted by the
# decision's number (its issue number if present, else its PR number).
#
# The index is NOT committed to git (zaplang-io/zap#1321): a single generated
# aggregate that every ADR-adding PR had to regenerate + commit was a standing
# merge-conflict hotspot (the same pathology the frozen DECISIONS.md had). CI no
# longer checks a committed copy — it VALIDATES the ADR sources instead (every
# ADR parses and carries the required frontmatter; no duplicate numbers). The
# browsable index is produced on demand by this script and by the docs site.
#
# Usage:
#   scripts/gen-decisions-index.sh                 # write decisions/INDEX.md (gitignored, for local browsing)
#   scripts/gen-decisions-index.sh -o -            # write the index to stdout (docs-site / piping)
#   scripts/gen-decisions-index.sh -o PATH         # write the index to PATH
#   scripts/gen-decisions-index.sh --validate      # validate the ADR sources only; no file written (CI gate)
#   scripts/gen-decisions-index.sh --check         # deprecated alias for --validate
#
# Host-safe: POSIX sh + awk + sort only, no LLVM, no network.
set -eu

# Resolve repo root from this script's location so it works from any CWD.
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$script_dir/.." && pwd)
dec_dir="$root/decisions"

# Argument parsing: default is "write to decisions/INDEX.md".
mode="write"
out="$dec_dir/INDEX.md"
while [ $# -gt 0 ]; do
  case "$1" in
    --validate|--check) mode="validate" ;;
    -o) shift; [ $# -gt 0 ] || { echo "ERROR: -o needs a PATH (or -)" >&2; exit 2; }; out="$1" ;;
    -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "ERROR: unknown argument '$1'" >&2; exit 2 ;;
  esac
  shift
done

# Fixed domain order; any other subdir is appended afterwards (sorted) so a new
# domain is still listed even before this list is updated.
ordered_domains="lang stdlib runtime tooling"

# Extract one frontmatter field ($2) from file ($1). Only the first YAML block
# (between the first two `---` lines) is consulted; the first match wins.
field() {
  awk -v key="$2" '
    NR==1 && $0=="---" { infm=1; next }
    infm && $0=="---"  { exit }
    infm {
      # split on the first ": "
      idx=index($0, ":")
      if (idx>0) {
        k=substr($0,1,idx-1)
        v=substr($0,idx+1)
        gsub(/^[ \t]+|[ \t]+$/,"",k)
        gsub(/^[ \t]+|[ \t]+$/,"",v)
        if (k==key) { print v; exit }
      }
    }
  ' "$1"
}

# ---------------------------------------------------------------------------
# Validate mode: the CI gate. Check the ADR SOURCES, not a committed artifact.
# Fails if any ADR is missing a required frontmatter field (title/status/date
# and at least one of issue/pr) — i.e. a field the index would render. Duplicate
# decision numbers are deliberately ALLOWED: one issue legitimately spans several
# stage ADRs (e.g. #1298), and the index lists each row, so this gate never
# blocks that.
# ---------------------------------------------------------------------------
if [ "$mode" = "validate" ]; then
  errfile=$(mktemp "${TMPDIR:-/tmp}/zap-decval.XXXXXX")
  trap 'rm -f "$errfile"' EXIT INT TERM
  count=0
  for f in $(find "$dec_dir" -mindepth 2 -maxdepth 2 -name '*.md' -type f 2>/dev/null | sort); do
    rel=${f#"$dec_dir"/}
    count=$((count + 1))
    for k in title status date; do
      [ -n "$(field "$f" "$k")" ] || echo "$rel: missing required frontmatter field '$k'" >> "$errfile"
    done
    issue=$(field "$f" issue); pr=$(field "$f" pr)
    [ -n "$issue$pr" ] || echo "$rel: missing both 'issue' and 'pr' (a decision needs a number)" >> "$errfile"
  done
  if [ -s "$errfile" ]; then
    echo "ERROR: ADR frontmatter validation failed:" >&2
    sort -u "$errfile" | sed 's/^/  - /' >&2
    exit 1
  fi
  echo "Validated $count ADR source file(s); all carry the required frontmatter."
  exit 0
fi

# ---------------------------------------------------------------------------
# Write mode: render the index as a derived view.
# ---------------------------------------------------------------------------
# Collect rows as: domain<TAB>key<TAB>status<TAB>date<TAB>title<TAB>relpath
rows=$(
  # Only decisions/<domain>/*.md — README/TEMPLATE live at the top level and
  # are therefore never matched.
  find "$dec_dir" -mindepth 2 -maxdepth 2 -name '*.md' -type f 2>/dev/null | sort | while IFS= read -r f; do
    domain=$(basename "$(dirname "$f")")
    title=$(field "$f" title)
    status=$(field "$f" status)
    date=$(field "$f" date)
    issue=$(field "$f" issue)
    pr=$(field "$f" pr)
    key="$issue"
    [ -z "$key" ] && key="$pr"
    [ -z "$key" ] && key="0"
    rel=${f#"$dec_dir"/}
    [ -z "$title" ] && title="(missing title: $rel)"
    [ -z "$status" ] && status="?"
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$domain" "$key" "$status" "$date" "$title" "$rel"
  done
)

# Build the index into a temp file, then route it to the chosen output.
tmp=$(mktemp "${TMPDIR:-/tmp}/zap-decidx.XXXXXX")
trap 'rm -f "$tmp"' EXIT INT TERM

{
  echo "# Decision index"
  echo
  echo "_Generated by \`scripts/gen-decisions-index.sh\` — a derived view, NOT committed to git."
  echo "The per-file ADRs under \`decisions/<domain>/\` are the source of truth._"
  echo
  echo "Historical decisions \`D-0001…D-0243\` live in the frozen"
  echo "[\`DECISIONS.md\`](../DECISIONS.md); new decisions are the per-file ADRs"
  echo "listed here (see [\`README.md\`](README.md))."
  echo

  present=$(printf '%s\n' "$rows" | awk -F'\t' 'NF{print $1}' | sort -u)
  emit_domain() {
    d="$1"
    block=$(printf '%s\n' "$rows" | awk -F'\t' -v d="$d" '$1==d')
    [ -z "$block" ] && return 0
    echo "## $d"
    echo
    echo "| # | Title | Status | Date |"
    echo "|---|-------|--------|------|"
    printf '%s\n' "$block" | sort -t"$(printf '\t')" -k2,2n | while IFS="$(printf '\t')" read -r dom key status date title rel; do
      printf '| [%s](%s) | %s | %s | %s |\n' "$key" "$rel" "$title" "$status" "$date"
    done
    echo
  }

  seen=""
  for d in $ordered_domains; do
    if printf '%s\n' "$present" | grep -qx "$d"; then
      emit_domain "$d"
      seen="$seen $d"
    fi
  done
  printf '%s\n' "$present" | while IFS= read -r d; do
    [ -z "$d" ] && continue
    case " $seen " in *" $d "*) : ;; *) emit_domain "$d" ;; esac
  done
} > "$tmp"

if [ "$out" = "-" ]; then
  cat "$tmp"
  rm -f "$tmp"
  trap - EXIT INT TERM
else
  mv "$tmp" "$out"
  trap - EXIT INT TERM
  echo "Wrote $out"
fi
