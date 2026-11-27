---
title: A dependency's import of its own nested submodule keeps the package-name prefix
status: Accepted
issue: 1697
pr:
date: 2026-11-27
domain: tooling
supersedes:
superseded-by:
---

# A dependency's import of its own nested submodule keeps the package-name prefix

**Status:** Accepted

## Decision

When the module linker (I#283/D-0081) resolves a dependency import whose leading
segment names the importing file's **own** package — i.e. the importing file
lives in that package's `src/` — it resolves the **full** dotted path under that
`src/` (`<name>.<sub>` → `<pkg>/<name>/src/<name>/<sub>.zap`), exactly as the
package resolves the same import when built standalone (tier 3). The
dependency-tier rule that strips the leading `<name>` segment
(`<name>.<sub>` → `src/<sub>.zap`) applies only to a **cross-package** import —
a *consumer* reaching into a different package. A package can therefore ship a
nested submodule at `src/<name>/<sub>.zap`, `import` it from its own code as
`<name>.<sub>`, and have it resolve identically whether the package is the root
project or is pulled in as a `zenv` path/registry dependency.

## Rationale

A package built standalone resolves its own `import <name>.<sub>` through the
local tier (tier 3): the full dotted path under the importing file's source
root, so `import validation.pattern` from `src/validation.zap` loads
`src/validation/pattern.zap`. That is the only way to address a nested submodule
standalone — the local tier never strips a prefix.

When the *same* package is consumed as a dependency, the linker loads its
`src/validation.zap` with that package in the dependency-search roots, so the
file's own `import validation.pattern` was captured by the dependency tier
first. That tier strips the leading `validation` segment and looked for
`src/pattern.zap`, which does not exist, so the dependency failed to compile with
`error[ZAP3010]: cannot find module 'validation.pattern' in dependency
'validation'` — even though it compiled cleanly standalone. The same source
file resolved its own import two different ways depending on whether it was the
root or a dependency. `zaplang-io/fastzap` hit this through its `validation`
dependency (fastzap#28); it blocks any dependency that ships nested submodules.

Keying the exception on *self-reference* (the importer physically lives in the
package's own `src/`, a canonicalized path equality) is the narrowest correct
fix: it makes a package's internal imports resolve identically in both roles and
touches nothing else. It is not a special case bolted onto the walk — it routes
the dependency's own imports through the same full-path semantics the package
already uses standalone.

Alternatives rejected:

- **Drop the prefix-strip for all dependency imports.** Would break the
  documented cross-package convention (`import aws_core.sigv4` →
  `aws_core/src/sigv4.zap`), exercised by `dependency_submodule_resolves`. A
  consumer and a package's own code address modules differently by design; only
  the self-referential case was wrong.
- **Make `copy_tree` / `zenv` materialization the fix.** The materialization
  walk already copies subdirectories recursively, so the nested file *is*
  present in `zenv/pkg/<name>/src/<name>/`; the issue's "does not materialize"
  framing was a symptom, not the cause. A regression test
  (`zenv_create_materializes_nested_path_modules`) pins the recursive copy; the
  real defect was in resolution, not materialization.
- **Let a consumer also reach a dependency's nested submodule via the full
  path.** Out of scope: nested submodules are a package's own internal structure,
  and the cross-package flat convention is unchanged. If a consumer needs a
  nested module exposed, the package re-exports it from a top-level module.

## Consequences

- A dependency may ship `src/<name>/<sub>.zap` nested modules and import them
  from its own code; they resolve when the package is a `zenv` path or registry
  dependency, not only standalone. This unblocks `fastzap` (fastzap#28) and any
  library with nested internal modules.
- The cross-package convention is unchanged: a consumer's `import <dep>.<sub>`
  still maps to `<dep>/src/<sub>.zap` (prefix stripped). The two conventions now
  coexist without the self-referential collision.
- A self-referential import that genuinely does not exist now reports
  `error[ZAP3010]: cannot find module '<path>' in package '<name>'` (naming the
  package rather than "in dependency"), pointing at the full path it expected.
- Verified host-side (module resolution is front-end only, no LLVM): the linker
  unit tests cover the direct nested self-import, a two-level nested chain, the
  unchanged cross-package flat case, and the "not found" diagnostic; a `zenv`
  materialization test covers the recursive copy.

See `decisions/tooling/1232-zap-test-module-resolution.md` (the shared
front-end module resolver this builds on).
