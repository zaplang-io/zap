---
title: argparse exposes the object parser's usage synopsis independently of help
status: Locked
issue: 1710
pr:
date: 2026-11-28
domain: lang
supersedes:
superseded-by:
---

# argparse exposes the object parser's usage synopsis independently of help

**Status:** Locked

## Decision

In the argparse object model (`crates/syntax/src/parser.rs`),
`ArgumentParser` gains a public `format_usage(self) -> str` method that returns
the usage synopsis on its own — the single `usage:` line, with no trailing
newline and none of the description or option/positional rows that
`format_help()` renders around it.

`format_help()` is refactored to lead with `self.format_usage()` rather than
build the synopsis inline, so the two share one builder and can never drift.
`format_help()` / `print_help()` output is byte-for-byte unchanged.

`format_usage()` is pure: it neither parses argv, mutates the parser, prints,
nor exits. It mirrors CPython's `ArgumentParser.format_usage()`.

## Rationale

- **A library needs the synopsis without the layout.** zaplang-io/cli#16 renders
  its own help (sections, annotations, wrapping, styles) from declaration
  metadata but wants to keep the core parser's exact usage syntax. Before this
  change the synopsis was trapped inside `format_help()`, mixed with the
  description and option rows, so the only ways to reuse it were to depend on the
  help layout (slicing the first line) or to re-implement a second synopsis
  builder that would inevitably drift. A dedicated method removes both.
- **Python parity.** CPython exposes exactly this split: `format_usage()` returns
  the usage string, `format_help()` returns the full message. Zap adopts the same
  name and the same "synopsis only" contract, less CPython's trailing newline,
  which the object model omits everywhere for its deterministic goldens.
- **One builder, no drift.** Extracting the synopsis into `format_usage` and
  having `format_help` call it is what guarantees the two stay identical — the
  walled-garden golden asserts `format_usage()` equals the first line of
  `format_help()`.
- **Pure prelude Zap, no new runtime, no seam signature.** The change is one new
  zero-argument method and a three-line refactor of `format_help`. Object-model
  methods bind natively; `crates/syntax/src/seam_sigs.rs` covers only the flat
  `argparse.*` module shim, and a no-argument method needs no entry there.

### Rejected alternatives

- **Leave callers to slice the first line of `format_help()`.** Couples the
  library to the help layout and breaks the moment the help message gains a
  prologue; also forces the caller to render (and discard) the entire help body
  just to read one line.
- **A `print_usage()` only (no `format_usage()`).** zaplang-io/cli#16 needs the
  string to compose, not a side effect on stdout. `print_usage` can be layered on
  later as a one-liner over `format_usage`; it is out of scope here.
- **A free `argparse.*` helper taking a parser.** The synopsis is parser state;
  a method keeps it discoverable next to `format_help`/`print_help` and matches
  Python.

## Consequences

- `crates/syntax/src/parser.rs`: new `ArgumentParser.format_usage(self) -> str`;
  `format_help` now appends `self.format_usage()` instead of building the
  synopsis inline. No new struct field, runtime function, or seam signature.
- Hidden arguments (#1702), grouped options, custom metavars, custom program
  names, and the subcommand selector all render in the synopsis exactly as they
  do in the help usage line, because it is the same builder.
- Unblocks zaplang-io/cli#16 (library-rendered help that reuses the core
  synopsis).
- New golden `tests/golden/argparse_object_usage.zap` covers plain options,
  store_true flags, positionals, a missing program name, the bare `-h`-only
  parser, custom metavars, hidden optionals and positionals, grouped options, the
  subcommand selector with a custom dest, the no-trailing-newline contract,
  purity (repeated calls and a subsequent parse), and the
  equals-first-line-of-`format_help` invariant.

<!-- Related: #997 (argparse object model, D-0238),
[lang/1702-argparse-hidden-options](1702-argparse-hidden-options.md),
zaplang-io/cli#16. -->
