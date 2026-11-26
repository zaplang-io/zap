---
title: "Route a built-in-named @derive to a prelude-provided @derivable generator"
status: Accepted
issue: 1596
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Prelude-provided `@derivable` (#1596)

**Status:** Accepted — the pipeline prerequisite for the Json retire (#1558,
epic #1551). It lets a `@derivable` trait + `interface` that live in the
**prelude** route `@derive(Trait)` in any module, with the user declaring
neither — the seam a built-in derive (`Json`) needs to become a library derive.

## The obstacle

`@derive(Trait)` for a library trait is wired by a **pre-sema** desugar,
`derive_synth::expand_library_derives` (`crates/driver/src/lib.rs`), which
synthesizes a forwarding stub per interface method to the registered generator.
It scanned only the **user module** for `@derivable`/`interface`. The prelude is
injected **separately and later**, inside sema and lowering via
`parser::with_prelude` (needle-gated by `references_prelude`). So a
`@derivable(Json)` placed in the prelude was invisible to the synthesis: no stub,
no routing. Every library-derive to date works only because its `@derivable` is
declared in the same user file.

## The fix (two halves, both inert until a prelude derivable exists)

1. **Prelude-aware synthesis.** `expand_library_derives` now also scans the
   prelude's declarations: it delegates to `expand_library_derives_with(module,
   extra)` with `extra = prelude_items()`. A prelude `@derivable`/`interface`
   registers like a user one; a same-named user declaration overrides it (the
   registry scans the prelude first, then the module). `_with` takes the prelude
   as a parameter so the prelude-awareness is unit-testable with a synthetic
   prelude.
2. **Generator-body injection.** `with_prelude` injects the whole prelude (as the
   needle surface does) when the module references a prelude `@derivable`
   generator — `references_prelude_derivable`, which reads the prelude's
   generator-function names and looks for a reference (the synthesized stub calls
   the generator by name). This brings the generator body and its `interface`
   into the sema/lowering module.

Both halves are **inert today**: the prelude registers no `@derivable`
(`prelude_derivable_generators()` is empty), so `references_prelude_derivable`
never fires and every current program stays byte-for-byte unchanged. Adding the
first prelude `@derivable` (the Json generator, #1558) activates the path.

## Verification

- `crates/driver/src/derive_synth.rs`:
  `a_prelude_provided_derivable_routes_a_derive_with_no_in_file_declaration`
  (a `@derivable`/`interface` supplied as the synthetic prelude routes a user
  `@derive` and strips the trait) and
  `a_user_declaration_overrides_a_same_named_prelude_derivable`.
- `crates/syntax/src/parser.rs`:
  `prelude_registers_no_derivable_today_so_the_hook_is_inert`.
- Validated end-to-end locally with a temporary prelude `@derivable` (reverted):
  a user `@derive` with no in-file declaration routed to the prelude generator,
  which injected and monomorphized (`_derive_…$UserType`) and lowered clean.

## Next

#1558 adds the prelude Json generator (encode+decode, structs / collections /
AWS hooks / open enums), flips `Json` Builtin→Library, and deletes `gen_json_*` /
`gen_open_enum_json` + the sema special-cases, gated on all Json goldens staying
byte-identical.
