---
title: Explicit boolean defaults for store_true / store_false flags
status: Locked
issue: 1692
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# Explicit boolean defaults for store_true / store_false flags

**Status:** Locked

## Decision

In the argparse object-model (`crates/syntax/src/parser.rs`,
`ArgumentParser._add`), a boolean flag (`action="store_true"` or
`action="store_false"`) now honors an explicitly supplied `default`:

| `action`       | `default` supplied | seeded absent value (`get_flag`) |
|----------------|--------------------|----------------------------------|
| `store_true`   | omitted (`""`)     | `"false"` → `False` (unchanged)  |
| `store_false`  | omitted (`""`)     | `"true"`  → `True`  (unchanged)  |
| `store_true` / `store_false` | `"true"`  | `"true"`  → `True`   |
| `store_true` / `store_false` | `"false"` | `"false"` → `False`  |
| `store_true` / `store_false` | anything else | catchable `ValueError` at `add_argument` time |

Previously `_add` unconditionally overwrote `default` with the action's implied
value, so any explicit `default` was silently discarded before parsing.

Behavior pinned by this decision:

- **Omitted default keeps the action default.** The `default=""` kwarg sentinel
  (`""` is never a valid flag value) means "not supplied", so the action's
  implied default is used exactly as before — fully back-compatible.
- **An explicit `"true"`/`"false"` seeds the absent value verbatim**, read back
  through `get_flag` (which compares against `"true"`).
- **An absent flag with an explicit default is NOT argv-present.** `_fill_defaults`
  seeds the value and marks `_present[dest] = False`, so `Namespace.has` stays
  accurate and distinguishes a defaulted flag from a supplied one.
- **Supplying a flag still applies its action**, consumes no value (#1508), and
  sets `has` — independent of the default.
- **An invalid default is a declaration error**, raised as a catchable
  `ValueError` from `add_argument` (not at parse time).

## Rationale

- **Python parity with a static adaptation.** CPython's `add_argument(...,
  default=...)` lets a `store_true`/`store_false` flag default to any object.
  Zap's `Namespace` stores values as `str` and `get_flag` reads `== "true"`, so
  the only values that round-trip through a boolean flag are the canonical
  strings `"true"`/`"false"`. Accepting exactly those seeds the documented
  default; rejecting the rest surfaces a programmer error instead of silently
  defaulting to `False` (the empty-string compare result).
- **Walled garden.** A bogus default (`"yes"`, `"1"`, `"True"`) is almost always
  a mistake; raising at declaration — eagerly, before any argv is seen — catches
  it at the call site rather than letting it masquerade as `False`.
- **Pure prelude Zap, no new runtime.** The change is three branches in `_add`;
  no runtime C, no seam-signature change (object-model methods get kwargs
  natively via I#481; `seam_sigs` covers only the flat `argparse.*` shim).

## Consequences

- `crates/syntax/src/parser.rs`: `ArgumentParser._add` branches on the flag
  default; `add_argument` doc comment documents the flag-default contract.
- Unblocks the boolean part of zaplang-io/cli#13 (a boolean default sourced from
  an opted-in environment variable, with explicit argv retaining priority).
- New golden `tests/golden/argparse_object_flag_defaults.zap` pins the full
  matrix (omitted/explicit/matching defaults, supplied-overrides, zero-arity
  value consumption, invalid-default raises, non-flag default untouched).

<!-- Related: #997 (argparse object model, D-0238), #1508 (zero-arity flags),
#1655 (Namespace getters), #1566 (catchable errors), zaplang-io/cli#13. -->
