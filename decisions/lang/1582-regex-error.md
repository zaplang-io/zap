---
title: RegexError (⊂ ValueError) raised by regex compile/substitution failures
status: Locked
issue: 1582
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# `RegexError` for an invalid regex pattern or replacement

**Status:** Locked

## Decision

Introduce a dedicated built-in exception type **`RegexError`**
(`ZAP_EXC_REGEX_ERROR = 23`, parent `ValueError`) and raise it from regex
pattern-compile and substitution failures, replacing the provisional bare
`ValueError` that #1566 (PR #1572) raised from `zap_regex_fail`
(`crates/codegen/src/runtime/str.inc.c`).

- **`RegexError`** — CPython parity note: Python's `re` module raises
  `re.error` (a subclass of `Exception`, not of `ValueError`); Zap instead
  parents the type under `ValueError` because #1566 established regex failures
  as value errors and SPEC §14 promised that a dedicated type, when chosen,
  *would subclass* `ValueError`. Subclassing `ValueError` is what makes this a
  non-breaking refinement: `RegexError` ⊂ `ValueError` ⊂ `Exception`.

Because it derives from `ValueError`, every existing handler keeps working with
no migration: `except RegexError` selects just the regex-failure case, while
`except ValueError` and `except Exception` still catch it. The message string is
unchanged (`"regex error: <what>: <pcre2 diagnostic>"`), so the uncaught-report
text and any message-matching callers are unaffected — only the exception *type*
is now specific.

```zap
def main():
    str s = "x"
    try:
        auto m = s =~ m/(/
    except RegexError:
        print("bad regex")          # dedicated type

    try:
        auto n = s =~ m/(/
    except ValueError:
        print("still a value error") # existing handlers keep working
```

An uncaught regex failure still reports the PCRE2 diagnostic and exits 1,
exactly as #1566 left it (and as the pre-#1566 direct `exit(1)` did before that).

## Rationale

- **Spelling: `RegexError`.** The issue asks for a dedicated, readable type.
  Python's own name is `re.error`, which does not read as a Zap exception name;
  `RegexError` matches the `...Error` convention of the rest of the built-in
  hierarchy (`ValueError`, `OSError`, `FileNotFoundError`, …) and says exactly
  what it is.

- **Parent `ValueError`, not `Exception`.** #1566 deliberately chose
  `ValueError` as the provisional type (a bad pattern is a kind of bad value),
  and SPEC §14 recorded that a dedicated type would *subclass* `ValueError`.
  Keeping that parent means the refinement is purely additive: no
  `except ValueError` handler written against the #1566 behaviour has to change.
  This diverges from CPython's `re.error` (which is flat under `Exception`), a
  conscious trade of exact-CPython-tree parity for backward compatibility with
  the shipped #1566 contract — documented here and in `PYTHON_DIFFERENCES.md`.

- **Mirror an existing type, touch every wiring site.** The new id is added at
  exactly the sites `FileNotFoundError` (#1311, D) occupies — the `ZAP_EXC_*`
  enum and `zap_exception_parent` in `zaprt.c`, sema's builtin name table and
  `is_exception_type`, lowering's `exception_type_id_name`, and the pyinterop
  name→id map in `str.inc.c` — so no exhaustive match (host or LLVM-only) breaks
  and the compile-time and runtime name resolutions agree. The regex lowerings
  (`zap_regex_match` / `zap_regex_match_all` / `zap_regex_subst`) already use
  `throwing_call` (invoke), so the typed raise unwinds into an enclosing
  `try/except` exactly as the previous generic raise did; no lowering change is
  needed beyond the name→id entry.

## Scope and consequences

- **Behaviour change (additive, non-breaking for `except ValueError`).** A regex
  compile/substitution failure now raises `RegexError` instead of the base
  `ValueError`. Because `RegexError` ⊂ `ValueError`, every existing
  `except ValueError` (and `except Exception`) handler is unaffected; only code
  that wants the finer distinction opts into the new name.
  `raise RegexError(...)` and the matching `except RegexError` clause are now
  available to user code.

- **`RegexError` is raised only from the regex surface.** It is the type for
  `zap_regex_fail` (invalid pattern on match/match-all, bad replacement group on
  substitution). Non-regex value errors keep raising `ValueError`.

- **Runtime id coordination.** `RegexError` takes id `23` (the next free id
  after `NULL_REFERENCE_ERROR = 22`). Any concurrent PR that also extends the
  `ZAP_EXC_*` enum must renumber on rebase so the id stays unique and contiguous.

See also D (`FileNotFoundError` ⊂ `OSError`,
`decisions/lang/1311-filesystem-notfound-error.md`), the parent #1566 decision
(`decisions/lang/1566-catchable-input-errors.md`) that made regex failures
catchable, and SPEC §9 (built-in exception hierarchy) / §14 (regular
expressions).
