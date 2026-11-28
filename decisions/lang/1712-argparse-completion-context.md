---
title: argparse exposes a read-only completion context for partial argv
status: Locked
issue: 1712
pr:
date: 2026-11-28
domain: lang
supersedes:
superseded-by:
---

# argparse exposes a read-only completion context for partial argv

**Status:** Locked

## Decision

`ArgumentParser` gains `completion_context(words, prefix) -> CompletionContext`
(prelude Zap, `crates/syntax/src/parser.rs`). Given the already-decoded words
*before* the cursor and the incomplete `prefix` being typed (`""` starts a new
word), it reports what the cursor word completes, reusing the same token
matching as `parse_args` — subcommand selection, value-taking options vs flags,
`--option=value`, and the `--` separator.

`CompletionContext` carries:

- `kind` — `"option"` (an option name), `"value"` (the value of `active_option`,
  with `value_prefix` the part typed so far), `"positional"` (the next
  positional `active_dest`), or `"subcommand"` (a child command).
- `command_path` — the chain of subcommands already selected.
- `options_allowed` — `False` once a `--` separator has been seen.
- `active_option` / `value_prefix` — the option being completed and, for an
  attached `--opt=prefix`, the value prefix split from the spelling.
- `active_dest` — the next positional destination (or the subparsers dest).
- `invalid` — `True` when a **completed** word (never the incomplete cursor
  word) was an unknown option or an invalid subcommand.
- `candidates` — declared completions matching the cursor prefix (option names,
  choice values, or subcommand names), a best-effort convenience.

The cursor word is treated as incomplete: a partial value is **never** validated
as a full choice, and the selected parser's required arguments are **never**
enforced. The query is pure — it never parses, prints, exits, invokes a handler,
or changes `parse_args` behavior.

Two refinements keep it faithful to the strict scanner and to visibility rules:

- **A completed word in a required-value position must be a real value (#1715).**
  When a value-taking option is pending, an option-like completed word or a `--`
  separator cannot satisfy it — strict `parse_args` raises "expected one
  argument" there — so the command line is marked `invalid` and the token is
  reprocessed as an option/separator rather than swallowed as the value. A lone
  `-` and a negative number are still accepted as values, and the *incomplete*
  cursor word in that position is still a valid value prefix.
- **Hidden arguments get no choice suggestions (#1716/#1702).** The pending-value
  and attached-value branches apply the same visibility policy as option-name
  completion: a `hidden=True` option parses normally but offers no `candidates`,
  whether addressed by exact spelling or a unique abbreviation. The same gate
  covers a **hidden positional** (a sibling of #1716 found by scanning the
  candidate-producing branches): it stays the completion target (`active_dest`)
  but suggests no choices.

## Rationale

- **Shell completion lives in the CLI package, argv grammar lives in core**
  (zaplang-io/cli#17). Candidate generation and the Bash/Zsh/Fish adapters belong
  to the library; what it cannot reproduce without duplicating the parser is
  core's token grammar — which parser the cursor is in and what the cursor word
  is editing. Strict `parse_args` rejects partial input (`["build", "--mode"]`
  raises "expected one argument"; a partial choice `"re"` is not a declared
  value), so the library needs a dedicated read-only query rather than a second
  copy of the scanner.
- **Reuses the core scanner.** The walk uses the same `_looks_option`,
  `_find_opt`, `_find_opt_abbrev`, `_is_zero_arity`, and `_find_sub` helpers as
  `parse_args`, so the two agree on token classification by construction.
- **Incomplete cursor word, never validated.** Completing `--mode re` must offer
  `release` even though `re` is not itself a valid choice, and
  `completion_context(["build"], "")` must not fail because `build`'s required
  positional is absent. Separating "a completed word was wrong" (`invalid`) from
  "the cursor word is incomplete" is exactly the distinction a completer needs.
- **Pure, in-prelude.** Like the rest of the object model it is plain prelude
  Zap with no runtime, sema, lowering, or seam change; `CompletionContext` is a
  plain data struct so a caller reads fields directly.

### Rejected alternatives

- **A lenient `parse_args` mode.** Overloading the strict parser with a
  completion flag would entangle two contracts (one that validates and may
  raise, one that must never validate the cursor word) and risks changing strict
  behavior. A separate read-only method keeps `parse_args` untouched.
- **Returning candidate strings only.** The library wants to form replacements
  itself (shell quoting, descriptions), so it needs the structured context —
  the selected parser path, the active option/value split, options-allowed — not
  just a flat candidate list. Candidates are provided as a convenience.

## Consequences

- `crates/syntax/src/parser.rs`: new `struct CompletionContext` and
  `ArgumentParser.completion_context`. No runtime symbol, seam signature, or
  sema/lowering change.
- Folded into the #1713 argparse-parity PR, reusing its scanner helpers.
- Unblocks zaplang-io/cli#17 (shell completion).
- New golden `tests/golden/argparse_object_completion.zap` covers a pending
  option value filtered by a partial choice, `--opt=prefix` splitting, a child
  option name after subcommand selection, an empty cursor word, a positional
  after `--`, a root option name, and an invalid completed word.

<!-- Related: #997 (argparse object model, D-0238),
[lang/1713-argparse-python-parity](1713-argparse-python-parity.md),
zaplang-io/cli#17. -->
