---
title: argparse options hidden from generated usage and help
status: Locked
issue: 1702
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# argparse options hidden from generated usage and help

**Status:** Locked

## Decision

In the argparse object-model (`crates/syntax/src/parser.rs`), argument
registration gains an optional named `hidden=False` parameter on
`ArgumentParser.add_argument` and on both group variants
(`ArgumentGroup.add_argument`, `MutuallyExclusiveGroup.add_argument`), threaded
through the shared `ArgumentParser._add` helper onto a new `Argument.hidden`
field.

`hidden=True` is **display-only**: the argument registers and parses *exactly*
as it would otherwise, but `format_help()` (and therefore `print_help()`) omits
it from both the usage line and the help rows. Specifically:

- A hidden flag or value option is dropped from the `usage:` line and from the
  `options:` section, in both root and subparser child parsers. A hidden
  positional is dropped from `usage:` and the `positional arguments:` section.
- A group heading (`add_argument_group`) is emitted once at its first **visible**
  member, so a group whose members are all hidden contributes no heading, and a
  leading hidden member does not shift the order of the visible rows.
- `-h, --help` is always shown.

Everything about parsing is unchanged for a hidden argument: seeded defaults,
typed-value validation, explicit-presence tracking (`has`), `required`
enforcement, and mutual exclusion all behave as for a visible argument. A
malformed explicitly-supplied hidden option and an unmet required hidden option
still raise their usual catchable diagnostics.

## Rationale

- **Python / Click parity.** CPython's argparse suppresses an argument from help
  with `help=argparse.SUPPRESS`; Click spells it `hidden=True` on an option. Zap
  adopts Click's spelling (a dedicated boolean, not a sentinel smuggled through
  `help`) because it keeps `help` a plain description string and reads the same
  as the consumer's mental model (zaplang-io/cli#15). Visibility is orthogonal to
  the help text, so it is its own parameter.
- **Display-only, never a parse change.** Hiding an argument must not change what
  the parser accepts — a compatibility flag that is accepted but undocumented is
  precisely the use case. The filtering therefore lives entirely in
  `format_help`; `_add`/`_parse_into`/`_check_required` treat `hidden` as inert.
- **Heading anchored to the first visible member.** Emitting a group heading only
  when a visible member exists (and anchoring the once-only heading to that
  member rather than the first-declared one) is what makes "a section with only
  hidden options has no heading" and "hidden members do not affect the order of
  visible rows" both fall out of one rule.
- **Pure prelude Zap, no new runtime, no seam signature.** The change is one
  struct field, one `_add` parameter, three `add_argument` signatures, and
  visibility guards in `format_help`. Object-model methods bind keyword/default
  arguments natively (I#481); `crates/syntax/src/seam_sigs.rs` covers only the
  flat `argparse.*` module shim, so it needs no entry for `hidden`.

### Rejected alternatives

- **A `help=argparse.SUPPRESS` sentinel (CPython spelling).** Zap has no
  `argparse.SUPPRESS` constant and `help` is a `str`; overloading it with a magic
  value would be less discoverable and would collide with a legitimate help
  string. Click's dedicated boolean is cleaner.
- **A separate `add_hidden_argument` method.** Doubles the surface (and the group
  variants) for what is one orthogonal attribute of an ordinary argument.

## Consequences

- `crates/syntax/src/parser.rs`: `Argument` gains `bool hidden`; `_add` gains a
  `bool hidden` parameter; the three `add_argument` signatures gain
  `hidden=False`; `format_help` filters hidden options/positionals from usage and
  help and anchors group headings to the first visible member;
  `__argparse_first_in_group` is now visibility-aware.
- Unblocks zaplang-io/cli#15 (compatibility flags and value options registered
  but omitted from generated usage/help).
- New goldens `tests/golden/argparse_object_hidden.zap` (parsing unchanged:
  defaults, typing, presence, required, mutual exclusion; and help omission) and
  `tests/golden/argparse_object_hidden_help.zap` (usage/help rendering across
  mixed and hidden-only groups, a hidden positional, and a subparser child).
  Host sema coverage in `crates/sema/tests/seam_kwargs.rs` pins `hidden=` as a
  known keyword on `add_argument` and the group variants.

<!-- Related: #997 (argparse object model, D-0238),
[lang/1692-flag-explicit-defaults](1692-flag-explicit-defaults.md),
[lang/1698-namespace-set](1698-namespace-set.md), #1508 (zero-arity flags),
zaplang-io/cli#15. -->
