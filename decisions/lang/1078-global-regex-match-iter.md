---
title: Global regex match `=~ m/.../g` yields an iterable `list[Match]` (eager)
status: Locked
issue: 1078
pr: 1109
date: 2026-11-19
domain: lang
supersedes:
superseded-by:
---

# Global regex match is an iterable of match objects

**Status:** Locked. Governing design is **D-0010** (`design/0001-perl-regex.md`,
"Captures — match objects, no globals"), which already specifies that
`subject =~ m/.../g` "returns an iterable of match objects". This ADR records the
concrete type and the eager-vs-lazy implementation choice that fixes #1078, where
the global form instead type-errored `ZAP4070: type 'Match' is not iterable`.

## Decision

The global match form `subject =~ m/PATTERN/g` (non-negated) has type
**`list[Match]`** — the ordered, non-overlapping matches of the pattern against
the subject, with Python `re.finditer` semantics. The single-match form
`subject =~ m/PATTERN/` is **unchanged**: it stays a bare `Match` (truthy on
success, `None` on no match). `!~` is unchanged (a boolean test; the `g` flag is
irrelevant to it).

- **Iteration & captures.** Because the type is an ordinary `list[Match]`, a
  `for mt in subject =~ m/.../g:` loop, list indexing, and `len(...)` all work
  with no new machinery, and each element is a `Match` on which `.group(0)`,
  numbered groups, and named groups (`.group("name")`) work exactly as on the
  single-match form.

- **Eager collection (not lazy).** All matches are collected up front into the
  `list[Match]` at match time, rather than stepping PCRE2 lazily on each
  iteration. Eager is simpler and GC-safe for a first cut: the list is a single
  GC object on the conservatively scanned C stack, so every match it holds stays
  live across the next allocation, and there is no iterator-state object to root
  or to represent in the type system. A lazy `MatchIter` (streaming, no
  up-front list) is a possible future optimization for very large subjects; it
  would be a representation change only, invisible at the `.zap` surface.

- **Zero-length / UTF-8 forward progress.** The runtime iterator advances past a
  non-empty match's end (keeping matches non-overlapping); after a **zero-length**
  match it steps forward one **UTF-8 code point** (not one byte) so the scan
  always makes progress and terminates — matching `re.finditer`'s span count on
  both ASCII (`m/a*/g` on `"xay"` → 4 spans) and multibyte input
  (`m/a*/g` on `"éa"` → 3 spans). An empty match at end-of-subject ends the scan.
  No match yields an empty list.

## Rationale

- **Why `list[Match]` and not a new iterator type.** D-0010 asks only for "an
  iterable of match objects". `list[Match]` already *is* iterable and already
  carries the element type through sema, lowering, and read-back, so it needs no
  new `Ty`, no new ZIR instruction, no new `inst_dest()` entry, and no new
  exhaustive-match arms (the classes of change that only an LLVM build catches).
  A dedicated `MatchIter` type would buy lazy streaming but cost all of that
  machinery for a feature whose common use (count / collect all matches, as in
  the regex-redux benchmark) materializes every match anyway.

- **Why eager over lazy.** The lazy form must hold live PCRE2 match state and a
  cursor across `.zap`-visible iteration boundaries (and across coroutine
  parking), which is exactly the kind of shared, safepoint-crossing mutable
  state D-0010 set out to avoid for match objects. Eager sidesteps it entirely.
  Eager's cost is O(all matches) memory up front; acceptable for a first cut and
  revisitable without a surface change.

- **Why reuse the single-match extractor.** Capture + name-table copying is
  identical to the single-match path, so it was factored into one
  `zap_regex_build_match` helper the single and global runtime entry points
  share — no duplicated extraction, and `.group*` behaves identically per match.

## Consequences

- `=~ m/.../g` is now truthy as a (possibly empty) list and iterable; a program
  that (incorrectly, pre-fix) relied on it being a bare `Match` did not compile
  before this change, so nothing regresses.
- No new user-callable method or signature is introduced (the form is an operator
  expression, not a seam function), so `crates/syntax/src/seam_sigs.rs` and
  `crates/sema/tests/seam_kwargs.rs` are unchanged.
- Implementation: sema types the global form as `list[Match]`
  (`crates/sema/src/lib.rs`); lowering emits `zap_regex_match_all` and types the
  result `list[Elem::Nested(Match)]` so read-back recovers `Ty::Match` for
  `.group` dispatch (`crates/ir/src/lower.rs`); the runtime adds
  `zap_regex_match_all` + a UTF-8 code-point step helper, reusing
  `zap_regex_build_match` (`crates/codegen/src/runtime/str.inc.c`); the symbol is
  registered in codegen (`crates/codegen/src/lib.rs`). Match objects are stored
  as traced pointer words in the list exactly like `list[str]` from `str.split`.

<!-- Governing design: D-0010 (design/0001-perl-regex.md). Read-back mechanism:
     Elem::Nested interner (#625). List-of-pointers precedent: str.split. -->
