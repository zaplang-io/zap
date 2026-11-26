---
title: "comptime: fold reflect.type_info(T) in a called generic helper (recursive element codecs)"
status: Accepted
issue: 1604
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Fold `reflect.type_info(T)` in a called generic helper (#1604)

**Status:** Accepted — a core comptime fix (epic #1248), maintainer-assigned. It
lets a **generic helper** (not only the top-level `@derivable` generator) resolve
`reflect.type_info(E)` for a type parameter `E` and recurse into a container's
element type, so a library codec can handle **nested collections** of arbitrary
depth (`list[list[T]]`, `dict[str, list[T]]`, `list[dict[str, T]]`) — the
built-in `@derive(Json)` surface from #1533.

## The gap

A derive generator inlines its per-field dispatch, and a nested-collection
element (`f.ty.elem` is itself a `list`/`dict`) would need the element codec to
**recurse** — which cannot be inlined to unbounded depth. Factoring the element
codec into a callable generic helper did not work, for two reasons:

1. **Kind dispatch didn't fold for a container-typed parameter.** The comptime
   type substitution handed to `select_comptime_if` / the comptime evaluator was
   a **name→name** map (`type_subst_names`), which dropped a parameter bound to a
   container (`list[int64]` has no bare name). So
   `if reflect.type_info(E).kind.name == "list"` in a helper never folded, and
   the `comptime` branch reached lowering (`comptime evaluation is not wired yet`).
2. **The element type couldn't be named as a type argument.** `reflect.type_info(E).elem`
   in `gen[reflect.type_info(E).elem](x)` was not resolved to a type, so the
   recursive call read as `unknown variable 'gen'`.

## The fix

1. **Full-type substitution.** `Evaluator::type_subst` becomes `HashMap<String,
   ast::Type>` (`with_type_subst` / `resolve_type_param_ty`), and mono hands the
   comptime `if` pruner the full `cur_subst` instead of a name-only projection. So
   `reflect.type_info(E)` resolves `E` to its concrete type — a named type, a
   generic instance, **or a container** — and `.kind` / `.elem` fold
   (`crates/comptime/src/{lib,eval}.rs`, `crates/mono/src/{lib,comptime_for}.rs`).
2. **Reflected type arguments.** `Mono::resolve_reflect_type_expr` resolves
   `reflect.type_info(E)` and its container accessors (`.elem` / `.inner` / `.key`
   / `.value`) to a concrete type against `cur_subst`, hooked into `expr_to_type`,
   so `gen[reflect.type_info(E).elem](x)` specializes the helper at the element
   type. This is the `expr_to_type`-level counterpart of the comptime
   `reflect.type_info` evaluator and of the #1575 reflected-type-argument rewrite.

Both are gated on an active substitution, so ordinary (non-generic) code is
untouched; the full host suite (comptime / mono / sema / ir / `zap_cli`) and the
existing derive/comptime goldens are unchanged.

## Scope — encode recursion; decode-container-construction is a follow-up

This enables the **read/encode** direction end-to-end (a recursive element
encoder over arbitrary nesting). The **decode** direction additionally needs
`reflect.construct` to build an empty container accumulator
(`reflect.construct(list[T])` → `[]`) whose element type pins an `auto` binding;
that is a separate `reflect.construct` widening + empty-literal typing follow-up
(tracked for the E5 retire #1558's nested-collection decode).

## Tests

- `tests/golden/comptime_recursive_element_codec.zap` — a library encoder
  recurses into `list[list[int64]]`, `dict[str, list[int64]]`,
  `list[dict[str, int64]]`, and `list[list[list[int64]]]`, byte-exact (CI LLVM).
- `crates/driver/tests/zap_cli.rs::comptime_reflect_folds_in_a_called_generic_helper`
  — a recursive element helper compiles (host, no LLVM).
