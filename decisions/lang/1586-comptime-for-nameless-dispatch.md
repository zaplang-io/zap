---
title: "comptime: f.ty.name is total (empty for nameless kinds), fixing comptime-for dispatch"
status: Accepted
issue: 1586
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# `f.ty.name` is total; fixes comptime-for dispatch over nameless types (#1586, #1584)

**Status:** Accepted — a core comptime fix surfaced by E6 serde breadth (#1552).
Resolves both #1586 and #1584, which share one root cause.

## The defect

Inside a `comptime for f in reflect.type_info(T).fields` loop, the unroller
resolves a comptime-constant `if`/`elif` chain by evaluating each condition: a
`Bool(true)` arm is kept, a `Bool(false)` arm is dropped, and **anything that does
not evaluate to a comptime boolean leaves the whole `if` as a runtime `if`**
(`crates/comptime/src/unroll.rs::prune_comptime_if`). Leaving it as a runtime `if`
means the loop variable `f` is no longer substituted inside it — so it leaks to
lowering as `error[ZAP6001]: unknown variable 'f'` (no span).

The trigger: `f.ty.name` **raised** a `CtError` for a nameless type kind — `list`,
`dict`, `Optional`, tuple — because `TypeInfo::name()` is `None` for those
(`crates/comptime/src/eval.rs`). So a perfectly ordinary dispatch like
`if f.ty.name == "int64": … elif f.ty.kind.name == "dict": …` blew up the moment a
field was a `dict` (its `f.ty.name` raised, the condition didn't fold, the chain
went runtime, `f` leaked). #1584 was the same cause one step on: the dropped-to-
runtime branch also held a `match`, which then lost `f` too.

This stayed latent because the shipped codecs (json/csv/yaml/serde) only ever
dispatched over **named** fields — primitives (named) and nested structs (whose
`f.ty.name` is the struct's own name, a real string that compares cleanly false).
Adding `list` / `dict` / `Optional` fields exposed it.

## The fix

`f.ty.name` is now **total**: it returns the type's own name for a named kind
(struct / enum / primitive) and the **empty string** for a nameless kind, instead
of raising (`crates/comptime/src/eval.rs`). So `f.ty.name == <literal>` folds to a
comptime boolean for *every* field kind, the comptime `if` chain is pruned, and
the loop variable is substituted throughout — including into a `match` nested in a
taken branch. A new `f.ty.is_named` accessor distinguishes a nameless kind from a
(non-existent) empty name, for code that needs the distinction; `f.ty.kind.name`
remains the way to dispatch on the structural kind.

No `Value::Null` exists in the comptime value model, and `value_eq` errors on
mismatched kinds, so an empty-string sentinel is the minimal total representation;
no type has an empty name, so dispatch stays correct.

## Tests

Two regression host tests in `crates/driver/tests/zap_cli.rs`:
`comptime_for_dispatch_is_total_over_nameless_field_types` (#1586 — a
`dict`/`list`/`int64` struct through an `f.ty.name`-first chain) and
`comptime_for_substitutes_loop_var_into_a_match_in_an_if_branch` (#1584 — a
`match` in an `elif` after an `f.ty.name` check on an `Optional` field). Both
previously failed with `unknown variable 'f'`; both now lower. Full
`zapc_comptime` / `zapc_mono` / driver suites stay green.

## Scope note

A `match` nested inside a *doubly*-nested comptime `if` (e.g. `if kind ==
"optional": if elem == "int64": match …`) remains a separate, narrower limitation
not covered here; it is not needed for the collection breadth this unblocks, and
single-level `match`-in-`if` (the #1584 shape) is fixed. This unblocks serde
`list` / `dict` breadth (#1552).
