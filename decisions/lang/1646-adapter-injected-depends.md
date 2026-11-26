---
title: Injected Depends parameters in the argparse-backed handler adapter
status: Locked
issue: 1646
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Injected Depends parameters in the argparse-backed handler adapter

**Status:** Locked

## Decision

The argparse-backed handler adapter (#1544/#1621/#1625/#1655) supports arbitrary
**injected** parameters via the existing D-0092 `Depends(provider)` marker — the
mechanism is **reused, not re-invented** (no new marker). An adapted handler may
therefore declare, in any position, a parameter whose default is
`Depends(provider)`; the adapter resolves it from its provider rather than from
the command line.

```zap
@cli.command
def greet(Context ctx, DB db = Depends(get_db), str name) -> str:  # db injected
    ...
# synthesizes (seam fn(Context, Namespace), unchanged):
def greet$adapter(Context $ctx, Namespace $ns) -> str:
    DB get_db$dep = get_db()                       # resolved deps-first, once each
    return greet$impl($ctx, get_db$dep, $ns.get_str("name"))
```

## Per-parameter classification (precedence)

Each handler parameter is classified, in this order:

1. default is `Depends(provider)` → **injected**: resolved from its provider in
   the adapter body (reusing `DepBuilder`: deps-first, one `<provider>$dep` local
   per distinct provider, nested `Depends` recursively, cycle-detected). Dropped
   from the namespace binding; **does not widen the seam**.
2. default calls a `@request_source` function (#959) → **error**: a command-line
   handler has no request value to resolve against. A precise decoration-site
   error, not a miscompile (use `Depends` or a command-line scalar instead).
3. the **first** parameter, when it is non-scalar and has no injection marker,
   under a context seam → the dispatcher-supplied `Context` (#1621, unchanged).
4. an argv scalar (`namespace_getter_for`) → bound from the `Namespace`
   (#1544/#1625/#1655).
5. any other non-scalar with no marker → the precise `ZAP5048`, unchanged.

The context stays strictly the handler's first parameter (#1621): a `Depends`
first parameter is injected (rule 1), leaving the dispatcher context argument
unused, exactly as a scalar first parameter does.

## Rationale

- **Reuse the decided mechanism.** `Depends`/`DepBuilder`/`classify_param` already
  exist and are decided (D-0092/#466); wiring them into the argparse adapter gives
  one injection vocabulary across both the registering (request) adapter and the
  argparse (CLI) adapter, with no new surface. The resolver is compile-time (no
  runtime reflection), so the adapter stays a statically-typed call.
- **Additive.** A handler with no `Depends` parameter is byte-for-byte unchanged;
  the `(C, Namespace)` seam is unchanged (injected parameters are resolved inside
  the adapter, never passed by the dispatcher).

## Consequences

- `crates/driver/src/decorator_desugar.rs`: `synth_namespace_adapter` takes
  `fn_sigs` + `source_fns`, classifies each parameter with `classify_param`, and
  resolves `Depends` via the shared `DepBuilder`; `try_build_rebind` threads
  `source_fns` through.
- Resolves the general-injected-parameter deferral in
  decisions/lang/1621-context-adapter.md.
- **Deferred (unchanged):** `@request_source` in a CLI handler (no request value);
  a framework-agnostic boxed-value seam (A-box).

<!-- Related: #1544/#1621 (adapter + context seam), D-0092/#466 (Depends),
#959/D-0198 (request sources), zaplang-io/cli#7. -->
