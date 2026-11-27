---
title: Replacing a resolved argparse Namespace binding (typed setters)
status: Locked
issue: 1698
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# Replacing a resolved argparse Namespace binding (typed setters)

**Status:** Locked

## Context

CLI parameter callbacks (zaplang-io/cli#14) are typed transforms — the Click
["callbacks for validation"](https://click.palletsprojects.com/en/stable/advanced/#callbacks-for-validation)
shape: they run after type conversion and validation and return the value the
handler should receive. The #1544 handler adapter reads each handler parameter
from the parsed `Namespace` by name through the type-matched getter. For a
callback's result to reach the handler, the callback must write that result
back into the same `Namespace`. The object-model `Namespace` (the embedded-Zap
prelude struct in `crates/syntax/src/parser.rs`) previously exposed only getters
(`get`/`get_str`/`get_int`/`get_float`/`get_flag`, the #1655 `get_char`/
`get_bytes`/`get_bigint`/`get_complex`) and presence queries (`has`,
`help_requested`); it had **no supported way to replace a resolved binding**, so
`values.set_int("jobs", 4)` failed with `ZAP6001: no method 'set_int'`.

## Decision

`Namespace` gains four typed setters that replace an **already-declared**
destination's resolved value in place:

| setter                         | stores                                          | read back by |
|--------------------------------|-------------------------------------------------|--------------|
| `set_str(name, str value)`     | the string verbatim                             | `get`, `get_str`, and — via the stored string — every other getter (`get_char`/`get_bytes`/`get_bigint`/`get_complex`) |
| `set_int(name, int64 value)`   | `str(value)` (canonical decimal)                | `get_int` (and the narrowing scalar reads built on it) |
| `set_float(name, float64 value)` | `str(value)` (canonical, like a `default`)    | `get_float` |
| `set_flag(name, bool value)`   | the canonical `"true"`/`"false"` string         | `get_flag` |

`set` alone is **not** offered: `set` is a reserved collection-type name, so the
string setter is spelled `set_str`. Because the whole `Namespace` is backed by a
`dict[str, str]` and every typed getter parses that stored string, `set_str` is
also the **universal** replacement — it is how a `char`/`bytes`/`bigint`/
`complex` binding (whose getters read the same string) is replaced, with no
dedicated setter each.

### Invariants

- **Declared-only.** The destination must already exist. `parse_args` seeds
  every declared argument via `_fill_defaults`, so any declared dest qualifies;
  a setter never *creates* a binding.
- **Unknown destination → catchable `KeyError`.** The documented typed error
  path, delegated through a private `_replace(name, value)` so the diagnostic
  lives in one place: `KeyError("no such argparse destination: '<name>'")`.
  `KeyError` is the natural missing-key signal (sibling of the getters' faults)
  and is catchable both generically (`except Exception`) and specifically
  (`except KeyError`), consistent with the #1566 catchable-error model.
- **Presence and help state are untouched.** Setters write only `_vals`, never
  `_present` or `_help`. So `has(name)` stays **true** for a value supplied on
  argv and **false** for a fallback/default — a replacement does not re-mark a
  value as "supplied".
- **Locality.** Only the named binding changes; sibling bindings and any
  separate parse (a distinct `Namespace`) are unaffected.
- **Subcommands.** Because a parent parser selecting a subcommand fills the
  *same* `Namespace` (`child._parse_into(ns, …)`), a child-sourced binding is
  replaced identically — no special case.

## Rationale

- **Pure Zap, no new runtime.** Each setter is a one-liner over the existing
  `dict[str, str]`, reentrant like the rest of the object model; no runtime C.
- **String-backed, symmetric with the getters.** The `Namespace` already stores
  every value (including declared `default`s) as a string and parses on read, so
  a typed setter that encodes back to that canonical string is the minimal
  design consistent with the whole model. `set_flag` stores the exact
  `"true"`/`"false"` token `get_flag` reads, never a raw `str(bool)`.
- **No new adapter surface.** The #1544/#1655 handler adapter only *reads* the
  `Namespace`; the setters feed that existing read path, so a callback's
  replacement is visible to a synthesized adapter with no change to
  `namespace_getter_for`.
- **Object-model methods, positional args.** Like the getters (ADR #1655), the
  setters take positional `name`/`value` and need **no** `seam_sigs` entry —
  `seam_sigs` covers only the flat `argparse.*` singleton shim, not the object
  model.

## Consequences

- `crates/syntax/src/parser.rs`: `Namespace` gains `_replace` (+ `KeyError`
  path) and `set_str`/`set_int`/`set_float`/`set_flag`.
- `tests/golden/argparse_object_set.zap` (+ `.expected`): exhaustive coverage —
  a supplied string, a default-sourced int, a declared boolean default, the raw
  `set_str` + `set_float`, sibling/separate-parse locality, a subcommand-sourced
  binding, and both the generic and typed (`KeyError`) unknown-destination path.
- Unblocks the transforming callbacks in zaplang-io/cli#14.

<!-- Related: #997 (D-0238 object model), #1544 (handler adapter), #1655
(non-numeric getters + adapter binding), #1566 (catchable errors),
zaplang-io/cli#14 (transforming callbacks). -->
