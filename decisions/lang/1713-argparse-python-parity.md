---
title: argparse object parser reaches CPython parity
status: Locked
issue: 1713
pr:
date: 2026-11-28
domain: lang
supersedes:
superseded-by:
---

# argparse object parser reaches CPython parity

**Status:** Locked

## Decision

Close the remaining CPython-`argparse` gaps in the object parser
(`argparse.ArgumentParser`, prelude Zap in `crates/syntax/src/parser.rs`) in one
change. Everything is pure prelude Zap on the `ArgumentParser` struct and its
free helpers; the module-facet constructor (`argparse.ArgumentParser(prog,
description)`), its sema/lowering, and `crates/syntax/src/seam_sigs.rs` are
unchanged, because object-method keyword/default arguments bind natively
(I#481). The legacy flat singleton API is untouched.

Added surface:

- **Option aliases (G1).** `add_argument("--verbose", alias="-v", ...)` — a
  second spelling. `dest` derives from the long spelling; either spelling
  matches during parse and both show in help (`-v, --verbose`).
- **`nargs` (G2).** `"?"` (uses `const_value`/`default`), `"*"`, `"+"`, a decimal
  count, and `"..."` (REMAINDER), on options and positionals. `*`/`+`/`N`/append/
  extend bindings read back with `Namespace.get_list` / `get_int_list` /
  `get_float_list`.
- **`parse_known_args` (G3)** → `(Namespace, list[str])`; **`parse_intermixed_args`
  (G18)**.
- **Action set (G4).** `store_const`, `append`, `append_const`, `count`,
  `extend`, `version` (+ `Namespace.get_count` / `version_requested`), alongside
  the existing `store`/`store_true`/`store_false`.
- **Short-option grouping & attached values (G5).** `-xvf`, `-ovalue`, `-o=value`.
- **`dest=` override (G7).**
- **Type tags (G8).** `bool`, `bigint`, `complex`, `char`, `bytes` added to
  `str`/`int`/`float`, validated at parse time; `choices` compares the converted
  canonical value (G10), so an int choice `"1"` matches the supplied `"01"`.
- **Parser-wide `argument_default` (G9)** via `set_argument_default`.
- **Subparsers (G11).** `add_subparsers(dest=, title=, description=, metavar=,
  required=)`; `add_parser(name, aliases=[...])`.
- **`set_default` / `get_default` (G12)**, including the subcommand-handler
  idiom adapted as a stored string.
- **`allow_abbrev` (G13).** Unambiguous long-option prefix matching; an ambiguous
  prefix raises `ambiguous option`.
- **Construction knobs (G14)** as chainable setters: `set_epilog`,
  `set_add_help`, `set_allow_abbrev`, `set_prefix_chars`, `set_formatter`,
  `set_conflict_handler`, `set_fromfile_prefix_chars`, `set_exit_on_error`
  (inert), `inherit(parent)` (CPython's `parents=`).
- **Help rendering (G15–G17).** Epilog; `choices` shown as `{a,b,c}` in usage and
  help; `%(default)s` / `%(prog)s` interpolation; argument-group descriptions
  (B3); a titled subparsers section.
- **`version` action (G19)** sets `version_requested()` and stores the version
  text as the destination value; no auto-print/exit.

Bug fixes found in the same scan:

- **B1.** The negative-number test is now CPython's `-\d+` / `-\d*\.\d+`, so
  `-1.5` / `-.5` bind as values (a `type="float"` option can take a negative
  value); exponent/hex forms remain option-like, exactly as CPython.
- **B2.** `required=True` on a positional is a declaration error (catchable
  `ValueError`).
- **B3.** `add_argument_group(description=...)` and a titled `add_subparsers`
  render their text instead of being silently ignored.

## Rationale

- **Static-language adaptations.** Zap's object methods cannot take CPython's
  `*name_or_flags` positionally alongside keyword arguments, and `action`/`type`
  are string tags rather than callables, so aliases use `alias=`, the action and
  type sets are extended tags, and `parents=` / the remaining constructor
  keywords become chainable setters. `const` is a reserved Zap keyword, so the
  keyword is `const_value` (field `const_val`). These are the minimum deviations
  needed; the behavior matches CPython.
- **No exit, no auto-print.** `-h/--help` and the `version` action set flags; the
  parser raises catchable errors instead of `exit()`. This pre-existing,
  deliberate divergence (a composable, testable library parser) is kept;
  `set_exit_on_error` is accepted for parity and is inert.
- **Help layout stays deterministic.** The simple two-space/two-space,
  no-wrapping layout is retained (it predates this change); only help *content*
  (epilog, choices metavar, placeholder interpolation, group/subparser
  descriptions) reaches parity. CPython's column alignment and width wrapping are
  intentionally not reproduced — they are terminal-width-dependent and would make
  the goldens non-deterministic.
- **One builder for usage.** `format_usage` (#1710) still leads `format_help`,
  now carrying nargs/alias/choices display, so the two never drift.
- **Pure prelude, native kwargs.** Because object-method kwargs bind natively
  (the #1702 `hidden=` precedent), none of this needs a seam signature, runtime
  function, sema facet, or lowering change.

### Rejected alternatives

- **CPython `const=` keyword.** `const` is a reserved keyword; `const_value` is
  the adaptation.
- **Constructor keywords for every knob.** The module-facet constructor binds two
  positional strings through sema/lowering; threading a dozen mixed-type
  keywords (and a `list[ArgumentParser]` for `parents`) through it is far more
  invasive than chainable setters that live entirely in the prelude.
- **CPython-exact help wrapping/alignment.** Width-dependent and
  non-deterministic; rejected for golden stability (see above).

## Consequences

- `crates/syntax/src/parser.rs`: `Namespace` gains list/count/version
  accessors and `_lists`/`_version` state; `Argument` gains
  `alias`/`action`/`nargs`/`const_val`/`version`/`is_list`; `ArgumentParser`
  gains the construction knobs, subparser metadata, group-description registry,
  and `set_default` seeding; `_parse_into` is generalized (extras, known,
  intermixed) with new `_consume_opt`/`_take_values`/`_take_positional`/
  `_bind_value`/`_bind_zero`/`_find_opt_abbrev` helpers; helpers gain
  `__argparse_choice_str`/`__argparse_nargs_disp`/`__argparse_interp`/
  `__argparse_subnames` and the B1 `__argparse_is_negnum` fix.
- No runtime symbol, seam signature, or sema/lowering change.
- Unblocks zaplang-io/cli (#15/#16/#17).
- New goldens `tests/golden/argparse_object_parity_{parse,decl,help,errors}.zap`;
  the existing `argparse_object_*` goldens are byte-for-byte unchanged.

<!-- Related: #997 (argparse object model, D-0238),
[lang/1702-argparse-hidden-options](1702-argparse-hidden-options.md),
[lang/1710-argparse-format-usage](1710-argparse-format-usage.md),
#1507 (`--` separator), #1508 (zero-arity flags), #1692 (flag defaults),
#1698 (namespace set), zaplang-io/cli#15/#16/#17. -->
