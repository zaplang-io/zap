# Proposal: seam calling convention — keyword / default argument binding for built-in module functions

- **Author(s):** Parvesh Garg
- **Last updated:** 2026-11-16
- **Status:** Accepted
- **Discussion:** #998 (epic #967); Discussion #965 (language-feature §, L-5)

## Abstract

Built-in ("seam") module functions — the hand-written check/lower pairs that
back `zlib`, `http`, `crypto`, `math`, … — were positional-only: they tolerated
keyword syntax but ignored the names, so a misspelled keyword on a stdlib call
was silently accepted and keyword reordering bound by position.
This proposal gives seam methods the same keyword / default / reordering
calling convention user functions already have, through a **single shared
signature registry** consulted centrally by both the sema and the lowering
dispatch, with the per-method check/lower bodies unchanged.

## Background

A seam method is dispatched through one funnel on each side —
`zapc_sema::modules::check_builtin_module_call` and
`zapc_ir::lower::modules::lower_builtin_module_call` — to a per-method function
that reads `args[i].value` **by position** and never consults `args[i].name`
(`crates/{sema,ir}/src/.../modules/*.rs`).
So there was nowhere to bind a keyword, no default to fill by name, and no name
to validate.
Concretely (verified with `zap ir`):

```zap
zlib.compress("hi", bogus=9)        # silently accepted — the typo is lost
zlib.compress(level=9, data="hi")   # bound by position → int in the data slot
```

User functions, by contrast, already do full name-based binding, defaults, and
unknown-keyword validation (`ZAP4023`).
This mismatch is why the stdlib fakes optionality with arity overloads
(`zlib.compress(data, level?)` via `args.get(1)`; `http.get` "n or n+1 args").

An earlier slice (#998, D-0203) proved the fix on one method by calling a binder
*inside* `zlib`'s `check_compress`.
This proposal generalizes that into a framework so every seam method can opt in
with a single line of data, keeping the per-method code clean.

## Proposal

A seam method declares its **positional parameter names** in one shared registry
and thereby gains the user-function calling convention:

```zap
zlib.compress(data="hi", level=9)       # keyword binding
zlib.compress(level=9, data="hi")       # reordering
zlib.compress(data="hi")                # optional `level` omitted, filled by name
http.get("http://x", headers=h)         # the former "n or n+1" overload, named
http.compress("hi", bogus=9)            # error[ZAP4023]: no parameter named 'bogus'
```

Binding rules mirror user functions and Python: positionals fill parameters left
to right, each keyword binds to the parameter of that name, omitted **trailing**
parameters are simply absent, an unknown keyword is `ZAP4023`, a duplicate value
or a positional-after-keyword is `ZAP4023`, and an omitted parameter that is
followed by a supplied one (an interior hole) is `ZAP4025`.
The result is the supplied arguments normalized to declared positional order, so
the per-method body — `args[0]`, `args.get(1)`, its own arity and type checks —
runs unchanged.

### Mechanism

Three parts (the binder/reorder logic shipped in D-0203; this adds the registry
and the central wiring):

1. **Registry — `zapc_syntax::seam_sigs`** (its own file, separate from the
   per-module code and from `zapc_syntax::seam`, which is the unrelated
   generic-construction sugar of D-0202). A flat table
   `SEAM_METHOD_SIGS: &[(module, method, &[param_name])]` with a
   `seam_method_params(module, method)` lookup. It lives in `zapc_syntax`
   because both `zapc_sema` and `zapc_ir` depend on it and neither depends on
   the other — the same twin-facet home pattern as D-0202.
2. **Sema** — `check_builtin_module_call` looks up the signature and, when
   present, runs `bind_seam_args` (reorder + default + validate) before calling
   the per-method `check`.
3. **Lowering** — `lower_builtin_module_call` looks up the signature and runs
   `reorder_seam_args` (a pure reorder; sema already validated) before calling
   the per-method `lower`, so a reordered keyword call emits the runtime call
   with arguments in the right slots.

Adding keyword/default support to a method is therefore a single registry row;
the per-method `check`/`lower` functions are not touched.

### Opt-in and incremental

A method with **no** registry row keeps the legacy positional-only behavior
(names ignored).
This is deliberate: seam calls that pass keywords today but rely on positional
binding are untouched until their method is given a row, so the change lands
with no regressions.
Migrated by the framework PR (#1008): `zlib`/`gzip` `compress`/`decompress`,
`zlib.crc32`/`adler32`, and all of `http` (client verbs + accessors); then
row-by-row across #1009: `datetime`, `random`, `base64`, `hex`, `hash`, `json`,
`yaml`, `logging`, `crypto`, `os`, `fs`, `time`, `process`, `sys`, `url`, `io`,
`signal`, `math`, `net`, and finally `linalg`, `tls`, `argparse`, `xml`,
`concurrent`, `bisect` (NumPy / Python / `ssl` parameter names).

A few modules are intentionally left positional-only rather than rowed:

- **`fft` and `threading` must never be rowed under this framework.** Their
  *lowerings* already read argument names directly (`fft` interprets `norm=` /
  `n=` / `d=`; `threading.Thread` reads `target=` / `daemon=` and folds the rest
  into the closure). The central reorder *strips* a keyword's name when it binds
  it to a slot (`bound.name = None`), so handing these methods a row would make
  their own name lookups fail — they are self-interpreting and stay on the
  legacy path by design.
- **`sync`, `deque`, `oset`** are low-level opaque-handle primitives whose only
  arguments are a receiver handle plus a scalar; keyword names there would be
  freshly invented, not Python-mappable. `deque`/`oset` are additionally in flux
  under the `int64`-handle rework (#983). Left positional pending a maintainer
  decision.
- **`gc`, `runtime`, `arch`** and every `_`-prefixed internal method are not
  user-facing surface.

The end state flips the default so a method with no row **rejects** unknown
keywords rather than ignoring them — killing the silent-accept footgun
everywhere. That flip waits until every keyword-taking method has a row *or* is
explicitly ratified as positional-only (the lists above being the proposed
end-state): the self-interpreting `fft`/`threading` and the handle primitives
stay no-row permanently, so the flip must treat "no row" as "reject unknown
keywords" only for methods not on that positional-only list.

## Rationale

- **Registry in `zapc_syntax`, not duplicated per facet.** Sema and lowering
  must agree on the parameter order or a reordered call miscompiles; a single
  table guarantees they cannot drift.
- **Central binding, per-method bodies unchanged.** The alternative — editing
  each of ~200 check/lower functions (as D-0203 did for one) — is more code and
  more risk. Hoisting to the funnel makes migration pure data.
- **Names-only rows (optionals trailing).** The binder rejects interior holes,
  so only trailing parameters may be omitted — which matches every current seam
  method (the trailing `level`/`headers`). A method with an interior optional
  would need a per-parameter `required` flag; none exists today, so it is a
  documented follow-up rather than speculative generality.
- **Transitional by design.** As modules move to real Zap/prelude functions
  (the #966 stdlib redesign), they inherit the user calling convention for free
  and drop their rows; the registry is the bridge until then.
- **No new grammar, no new diagnostic codes.** Keyword arguments are already
  grammatical; this assigns meaning to the names on a seam call and reuses the
  user-function codes (`ZAP4023` name-binding, `ZAP4025` arity) so a seam
  mistake reports identically to a user-function mistake.

## Compatibility

Positional seam calls are byte-for-byte unchanged (the binder fast-paths a
call with no keyword arguments and passes spread `*`/`**` calls straight
through). Only keyword calls on migrated methods change — from
silently-ignored-name to bound-and-validated. `http` is Experimental and
`zlib`/`gzip` keyword calls had no defined meaning before, so there is no stable
behavior to break.

## Testing

- Host (parse + sema), `crates/sema/tests/seam_kwargs.rs`: bind-by-name,
  reordering, positional+keyword mix, unknown/duplicate/missing rejected,
  default-by-name, `gzip` shares the path, `http` client verbs with the optional
  `headers`, positional calls unchanged, wrong-typed keyword still `ZAP4024`.
- Registry unit tests (`zapc_syntax::seam_sigs`): lookup, unique rows, distinct
  parameter names per method.
- Lowering reorder verified on host via `zap ir`.
- CI (full/arm64): the `zlib_gzip_kwargs` golden round-trips keyword, reordered,
  defaulted, and keyword-`decompress` calls byte-identically to the positional
  form; the `bisect_kwargs` golden proves a reordered keyword call (`x=` before
  `a=`) mutating the slot-0 list in place matches the positional golden exactly.

## Follow-ups

- Registry rows are now populated for every user-facing keyword-taking seam
  module (see "Opt-in and incremental"); the only remaining no-row methods are
  the self-interpreting (`fft`/`threading`) and handle-primitive
  (`sync`/`deque`/`oset`) ones held as positional-only.
- Flip the no-row default to reject unknown keywords once the positional-only
  list is ratified (and `deque`/`oset` settle under #983).
- A per-parameter `required`/interior-optional model if a future method needs a
  non-trailing optional.
- Converge with #966 (seam methods as real Zap/prelude functions).

See `DECISIONS.md` D-0204 (this framework) and D-0203 (the one-method proof it
generalizes).
