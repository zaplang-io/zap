# Proposal: Zap-to-Zap module & package imports

- **Author(s):** zap-module-imports-283 (Claude)
- **Last updated:** 2026-10-28
- **Status:** Under review
- **Discussion:** zaplang-io/zap#283 (parent D-0028, D-0052, D-0011 §8)

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

Zap programs cannot yet span more than one file: `import` resolves only
compiler-known stdlib modules (D-0056) and `import python` (D-0028/D-0012),
and project mode compiles exactly one entry file (`src/main.zap`).
This proposal concretizes **D-0028** ("one file = one module; a directory = a
package; everything public except `_`-prefixed names") and **D-0052** (logical
dependency names resolved through `zenv`) into the compiler front end, so a
program can use `def`/`struct`/`enum`/`interface` declared in a sibling file and
a library package can be imported by a program.
It specifies how a dotted import path resolves to a file, how intra-project
imports differ from cross-package (dependency) imports, how visibility is
enforced, how the import graph is discovered and compiled, and how the whole
thing maps onto the AWS SDK's `aws-core` + per-service package shape (D-0011
§8).

## Background

Today the pipeline is single-module end to end
([`crates/driver/src/lib.rs`](../crates/driver/src/lib.rs) `compile_for` →
`front_end_for` → `lower_module` → `build_executable`): one source file becomes
one AST `Module`, one ZIR module, one LLVM module, one object, linked with the C
runtime.
`resolve_entry` ([`crates/driver/src/cli.rs`](../crates/driver/src/cli.rs))
selects `src/main.zap` in project mode and reads nothing else.

The grammar and parser already accept the syntax.
`docs/GRAMMAR.ebnf` has `import import_list` with dotted names, aliases, and
comma/grouped lists (I#70), and the AST carries
`Import::Module { path, alias, span }`.
But `Import::Module` is a **no-op** through sema, ir, and codegen: the parser
desugars a multi-import into one `Import::Module` per module and nothing
consumes them (see the I#70 and I#121 notes).
Stdlib member access (`math.sqrt`) resolves by the *literal* base identifier in
`crates/sema/src/modules/` — the `import` line is "accepted but optional" and is
never bound to anything.

The package manager (I#69, D-0052) already materializes dependencies into a
project-local cache and exposes where they live
(`zapc_pkg::zenv::module_search_paths_from_env()` → `$ZAP_ZENV/pkg/<name>`), and
the registry read path (I#236, D-0027) feeds the same resolver.
What is missing is the **front end consuming any of this** for Zap code: turning
`import a` into "find, parse, type-check, and link module `a`."

Two locked decisions already fix the *model*, so this proposal is mostly about
pinning the mechanical resolution and compilation details and surfacing the
genuinely open choices:

- **D-0028 (Locked):** one file = one module; a directory = a package;
  Python-like `import` + aliasing (`import x as y`); **visibility** is public for
  everything except names beginning with `_`, which are private and
  **enforced** at compile time. Zap has **no** `from x import y` form (I#70).
- **D-0052 (Locked):** source uses clean **logical names**; `zap.toml` maps each
  to a VCS/path/registry source + semver; MVS resolves them into a project-local
  module cache under `zenv/pkg/<name>`.

`docs/STABILITY.md` lists "modules/imports/visibility" as **Stable (S)** and the
1.0 spec-freeze (D-0076) ratified it as Stable, so the observable resolution and
visibility semantics specified here become part of the 1.0 contract.
(Internal compilation strategy — §Implementation — is **not** observable and
stays changeable.)

## Proposal

### 1. A module is a file; a package is a directory with a manifest

A **module** is one `.zap` file.
Its public surface is every top-level `def`, `struct`, `enum`, and `interface`
whose name does **not** begin with `_` (D-0028).
A **package** is a directory containing a `zap.toml` manifest (D-0052) and a
`src/` tree of modules; the project you compile is itself the *root package*.

Members are always accessed **qualified** through the imported name, exactly as
stdlib is today (`a.helper()`, `a.Widget`); there is no unqualified/`from`-import
form (D-0028, I#70).

```zap
# src/geometry.zap
def area(w: int64, h: int64) -> int64:
    return w * h

def _clamp(x: int64) -> int64:   # private: leading underscore
    return x

# src/main.zap
import geometry

def main():
    print(geometry.area(3, 4))   # 12
    # geometry._clamp(1)         # error ZAP43xx: `_clamp` is private to `geometry`
```

`import geometry as g` binds `g` instead of `geometry` (D-0028).

### 2. Path resolution

An import path is a dotted name (`a`, `a.b.c`).
Resolution picks a **search root**, then maps the remaining path to a file
*within that root's `src/`*:

within a root, `x.y.z` → `<root>/src/x/y/z.zap`, and a bare `x` → `<root>/src/x.zap`.

The root is chosen by the **first segment**, in this fixed precedence:

1. **Compiler-known stdlib module** (`math`, `os`, `json`, … — the names
   registered in `crates/sema/src/modules/`, D-0056). Existing behavior,
   unchanged; the `import` line stays optional.
2. **Dependency package.** If the first segment equals a dependency **logical
   name** (from `zap.lock`/`zap.toml`, D-0052), the root is that package
   (`$ZAP_ZENV/pkg/<name>`), and the **remaining** segments map within it. A
   bare `import <name>` selects the package's root module `src/<name>.zap`.
3. **Local project module.** Otherwise the **whole** path maps within the
   *importing file's own package* source root. In the root package that is the
   project's `src/`.

Intra-package imports are therefore resolved **relative to the importing file's
package**: a file inside `aws-core` that writes `import sigv4` gets
`aws-core/src/sigv4.zap`, not something in the top-level project.
A first segment that matches a dependency logical name "escapes" to that
dependency.

| statement (in project `src/main.zap`) | resolves to |
|---|---|
| `import geometry` | `src/geometry.zap` |
| `import shapes.circle` | `src/shapes/circle.zap` |
| `import aws_core` (dep) | `$ZAP_ZENV/pkg/aws_core/src/aws_core.zap` |
| `import aws_core.sigv4` (dep) | `$ZAP_ZENV/pkg/aws_core/src/sigv4.zap` |
| `import math` | stdlib (compiler-known) |

A local file whose name collides with a stdlib or dependency name is **shadowed**
by the higher-precedence root; the compiler emits a warning pointing at the
collision (reserved-name guidance), because stdlib/dependency names are part of
the stable surface and must win deterministically.
A path that resolves to no file, or to a directory with no matching `.zap`,
is a hard error with the paths tried.

### 3. Visibility

Public = top-level name not starting with `_`; private = starting with `_`
(D-0028), enforced:

- Qualified access to a private member of another module (`m._x`) is a compile
  error (new `ZAP43xx`).
- `_`-names remain freely usable **within** their defining module.
- Visibility is per *name*, not per *kind*: it applies uniformly to functions,
  structs, enums, interfaces, and their top-level aliases.

There is **no** export marker and **no** re-export form in v1; a package exposes
exactly the public names of the modules a consumer imports.
(`__init__`-style package aggregation / re-export is a deferred extension,
§Open issues.)

### 4. Entry point, the import graph, and non-entry modules

The entry is unchanged: `src/main.zap` in project mode, or the explicit file for
`zap run f.zap`.
From the entry the compiler **discovers the import graph** — it transitively
reads `Import::Module` statements, loading each module file **once** — and
compiles the closure together.

**Non-entry modules are declaration-only.**
A top-level *executable statement* (an `Item::Stmt`) in an imported module is an
error (new `ZAP43xx`); only the entry module's top level runs (via `main`).
This is the load-bearing simplification: it means importing a module has **no
import-time side effects and no initialization order**, so the merge-compile
strategy (§Implementation) is sound and cycles are a pure naming question rather
than an ordering one.
It also matches the library use case exactly (aws-core is declarations).

**Circular imports.**
v1 **rejects** an import cycle (`a → b → a`) with a clear diagnostic
(`ZAP43xx`, naming the cycle path).
Because non-entry modules are declaration-only, lifting this to *allow* cycles
is a sound future extension (whole-program name resolution has no ordering
hazard); it is called out as an open question so the maintainer can pick
reject-v1 vs allow-now.

### 5. Mapping onto the AWS SDK (D-0011 §8)

- `aws-core` is a **package**: a directory with `zap.toml` (`[package] name =
  "aws_core"`) and `src/` holding the signer, creds, endpoints, transport,
  retry, protocol, pagination, and error modules (e.g. `src/sigv4.zap`,
  `src/creds.zap`, …), importing each other with bare intra-package imports
  (`import sigv4`).
- `aws-s3` and `aws-dynamodb` are **per-service packages**, each declaring
  `aws_core` as a dependency in its `zap.toml` (path source for local dev, git
  or registry otherwise, D-0052/D-0027) and doing `import aws_core` /
  `import aws_core.sigv4`.
- The botocore-model generator emits one service module per package on top of
  the shared core — the shape it was designed for.
- The `tools/bundle.py` concatenation workaround (FRICTION.md #1) is retired
  once this lands.

This needs **no** new package-manager or registry surface: cross-package
resolution is just `module_search_paths_from_env()` selecting the search root
in §2 rule 2.

## Rationale

- **Follow the locked model, don't invent one.** The resolution/visibility shape
  is D-0028; the dependency-name/`zenv` shape is D-0052. This proposal is their
  concretization, not a competing design. The genuinely new content is the
  dotted-path→file mapping, the precedence order, the declaration-only /
  cycle rules, and the compilation strategy.
- **Qualified-only access (no `from`-import)** matches the stdlib precedent
  (`math.sqrt`) and the existing grammar, and keeps resolution a simple
  name→module→member lookup.
- **Declaration-only non-entry modules** is chosen over Python's "run the module
  body on import" because Zap compiles AOT to a single binary with no module
  object protocol; defining per-module init order and import-time side effects
  would be a much larger, ABI-shaped commitment for no v1 benefit (libraries are
  declarations). Python-like *lazy, ordered* init can be revisited if a real use
  case appears.
- **Precedence stdlib > dependency > local** keeps the stable stdlib and the
  lockfile-pinned dependency names deterministic and un-shadowable by a local
  file, which matters because those names are 1.0-stable surface.

## Compatibility

- Purely additive to **observable** behavior: today `import <zap module>` is a
  silent no-op, so no existing single-file program changes meaning. Programs
  that currently *rely on the no-op* (an `import a` that was ignored) will now
  attempt real resolution; this is the intended fix, and a previously-ignored
  import that resolves to nothing becomes a (correct) error.
- No IR schema or runtime-ABI boundary is touched (§Implementation keeps one
  LLVM module). No new external dependencies.
- Stable-surface addition under D-0076: the resolution and visibility semantics
  above are the specified behavior for the already-Stable "modules/imports/
  visibility" line in `docs/STABILITY.md`.

## Implementation

Staged; the first stage alone closes the #283 repro and unblocks the AWS SDK
local split.

**Stage A — intra-project local modules (the non-contentious core).**
1. **Loader + graph (driver).** New module-graph builder: from the entry, parse
   each file, scan `Import::Module`, resolve via §2 rule 3 (project `src/`),
   load-once, detect cycles (§4), collect the closure. Extend `front_end_for`
   to operate on the set.
2. **Sema.** Bind each imported name to its module; resolve qualified access
   `a.member` to that module's public declaration; enforce `_`-privacy
   (`ZAP43xx`) and the declaration-only rule (`ZAP43xx`).
3. **Compile strategy — merge into one AST `Module` before lowering.** Each
   imported module's top-level symbols are rewritten to module-qualified
   internal names and qualified references are rewritten to match; the entry's
   `main` is untouched. The merged module flows through the **existing**
   `lower_module` → `build_executable` path unchanged.
   - This keeps AOT a **single LLVM module**, so the D-0080 JIT internal-linkage
     behavior (REPL MCJIT only) is **not involved** and AOT linking stays correct
     by construction.
   - It also keeps **generics** working: a generic defined in one module and
     instantiated in another is monomorphized within the one module, exactly as
     today. Separate-object linking (rejected for v1) would force a cross-object
     symbol ABI and cross-module monomorphization — a far larger, ABI-committing
     change for no observable gain. The strategy is internal and can change later.
4. **Diagnostics.** Allocate the new `ZAP43xx` codes (private access, top-level
   statement in non-entry module, circular import, unresolved module) per D-0071.
5. **Golden harness.** Extend `crates/driver/tests/golden.rs` to support a
   project-directory fixture (a dir with `src/` + multiple files + one
   `.expected`), since today it runs one `.zap` per fixture.

**Stage B — cross-package (dependency) imports.**
Add §2 rule 2: first segment = dependency logical name → root =
`module_search_paths_from_env()` entry for `<name>`. Reuses Stage A's loader and
merge-compile wholesale; only root selection differs. Gated on a resolved `zenv`
(the pkg manager already materializes it). This is the part most coupled to
D-0052/D-0027 and is where review matters most.

**Tests:** the #283 repro (`a.zap` defines `helper`, `b.zap` imports+uses it);
`_`-privacy violation; declaration-only violation; circular-import diagnostic; a
two-package shape mirroring `aws_core` + one service (Stage B).
**Docs:** `docs/GRAMMAR.ebnf` (drop the "impl staged" note), `docs/STABILITY.md`
(point the modules/imports/visibility line at this doc), and a DECISIONS entry.

## Open issues

- **Circular imports: reject (v1 proposal) vs allow now.** Declaration-only
  modules make allowing them sound; rejecting is simpler to explain and gives a
  crisp error. Maintainer's call.
- **Package root-module convention: `src/<name>.zap` (proposed) vs `src/lib.zap`
  vs requiring submodule imports.** Affects what bare `import <name>` of a
  dependency binds.
- **Re-export / package aggregation** (a package presenting a curated surface
  rather than its raw file layout) — deferred; worth deciding before many
  packages exist.
- **Shadowing a stdlib/dependency name with a local file**: proposed as a
  warning + higher-precedence-wins; could instead be a hard error.
- **Top-level execution in non-entry modules** is forbidden in v1; revisit only
  if an ordered-init use case appears.
