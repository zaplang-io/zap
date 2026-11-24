---
title: "E4 shadow-proves @derive(Repr); reflect.type_info(T).name folds as a value"
status: Accepted
issue: 1430
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Shadow-prove `@derive(Repr)` + fold `reflect.type_info(T).name` (#1430)

**Status:** Accepted — the E4 (#1429) dogfood slice for `Repr`, unblocked now
that `repr()` is a builtin (#1384, merged via #1442). Same shadow approach as Eq
(`decisions/lang/1431-derive-eq-shadow-prove.md`): a library `@derivable`
generator expresses the derive and a behavior-identical golden shows it agrees
with the built-in, **without replacing** it.

## Small enabler: `reflect.type_info(T).name` as a value

A `Repr` generator must name the type (`TypeName(...)`). The comptime engine can
evaluate `reflect.type_info(T).name`, but monomorphization only folded
`reflect.type_info(T).<member>` in two positions — a `comptime for` iterator and
a kind-dispatch `if` condition — so used as a plain value it reached lowering as
an unresolved `reflect` (`ZAP6001: unknown variable 'reflect'`). This slice adds
a targeted fold in `rewrite_attr` (mono): when the base is `reflect.type_info(T)`,
the member is `name`, and `T` is substituted to a concrete type (an active
instantiation), it rewrites to that type's name as a string literal. A
still-generic `reflect.type_info(T)` falls through unchanged, so nothing else is
affected (the expression previously only errored).

## Shadow generator

```zap
@derivable(StructRepr)
def _derive_structrepr[T](T value) -> str:
    str out = reflect.type_info(T).name + "("
    comptime for f in reflect.type_info(T).fields:
        if comptime f.index > 0:
            out = out + ", "
        out = out + f.name + "=" + repr(reflect.field(value, comptime f.index))
    return out + ")"
```

`repr(x)` lowers to `render_value(.., RenderMode::Repr)` — the **same** protocol
the built-in `Repr` uses for each field (`render_repr`), and `repr(struct)`
resolves `__repr__` first. So the shadow's `TypeName(f=repr(field), …)` is
byte-identical to the built-in's `gen_struct_repr_body`, including str/char/bytes
quoting and nested-struct recursion.

## Guard

- Golden `tests/golden/derive_repr_shadow.zap`: `repr(p) == p.structrepr()` for
  two structs (int/str/bool fields + a nested derived struct), plus
  `repr(p) != q.structrepr()` to show the rendering is non-vacuous (runtime
  output, CI LLVM matrix). Asserts agreement, not a fixed repr string.
- Host test `a_library_generator_shadows_the_built_in_repr_without_replacing_it`:
  both `@P____repr__` (built-in, untouched) and `@_derive_structrepr$P` emitted,
  with the type name folded to `strconst "P"`.

No derive wiring changes; the built-in `gen_repr_method` and the default renderer
are untouched.

## Consequence

Eq, Ord, Hash, Repr are all shadow-proved. The remaining E4 derives are the
codecs (Json/Csv/Yaml), which need core enablers before they can be shadow-proved
(a reflect construct/field-set surface for decode; an ordered-object JSON build or
a source JSON-escape for encode).
