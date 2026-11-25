---
title: "Desugar reflect.field / reflect.set_field inside a match case arm"
status: Accepted
issue: 1506
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Desugar reflect intrinsics inside a `match` arm (#1506)

**Status:** Accepted — a localized bug fix. A reflection intrinsic
(`reflect.field` / `reflect.set_field`) used **inside a `match` case arm** of a
`@derivable` generator body was not desugared, so the `reflect` receiver leaked
to lowering and the program failed with a span-less
`error[ZAP6001]: unknown variable 'reflect'`.

## Root cause

The generator-body desugar walk, `desugar_stmt_fields`
(`crates/mono/src/comptime_for.rs`), rewrites `reflect.field(value, comptime i)`
to a typed field read and `reflect.set_field(target, comptime i, v)` to
`target.<name_i> = v` once the `comptime for` is unrolled and each field index is
a concrete constant. It recursed into the nested blocks of `if` / `while` / `for`,
but **every other block-bearing statement — `match` among them — fell through to
`other => other.clone()`**, which copies the statement verbatim. So a reflect
call inside a `match` arm survived un-desugared; the `reflect` name then resolved
to nothing at lowering.

This surfaced while writing a DOM codec decoder for #1490: the natural shape is
to `match` the parsed value and populate a field from within an arm. It was
side-stepped there (compute the value into a local, call `set_field` after the
`match`), but the desugar gap itself is the defect fixed here.

## Fix

Add a `Stmt::Match` arm to `desugar_stmt_fields` that desugars the scrutinee
expression and each arm's optional guard, and recurses into every arm body
(through `desugar_block_fields`, so an arm-local `reflect.set_field` is handled by
`desugar_set_field` on the inner statement exactly as at the top level).
Patterns bind names only and carry no reflect call, so they are kept as-is.

A codec decoder can now write `reflect.set_field` / `reflect.field` directly
inside a `match` arm, not only before/after the `match`.

## Scope

Fixes the reported `match` case. Other block-bearing statements that the same
walk still does not descend into — `select`, `try` / `except` / `finally`,
`with`, `unsafe`, `scope`, `do-while` — remain a latent coverage gap, but no real
generator reaches a reflect intrinsic through them today; widening the walk to
those is deferred until something needs it.

Unrelated and **not** addressed here: `reflect.field(x, 0)` written with a *bare
integer literal* index (rather than `comptime f.index`) is not recognized as the
intrinsic regardless of a `match`; that is a separate limitation of the
index-argument form, not of the statement walk.
