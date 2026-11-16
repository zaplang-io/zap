//! The `arch.*` low-level-intrinsics module family (I#139 Tier 2, #962, D-0199).
//!
//! `import arch.x86_64 as x86` / `import arch.aarch64 as a` exposes the curated,
//! architecture-specific CPU intrinsics (`crc32c_*`, `popcount_*`, `prefetch`)
//! by their familiar C names, each lowering to the matching hardware
//! instruction. The family is **off the default surface** (an explicit `arch.*`
//! import) and honours D-0079's "no silent fallback" rule: `import arch.x86_64`
//! is a hard compile error when the build does not target x86-64, and
//! `import arch.aarch64` likewise off aarch64 (enforced in the driver's module
//! linker, which is where the active [`crate::cfg::Target`] is in scope).
//!
//! The call seam dispatches a built-in module call on the *receiver name*
//! (`x86.crc32c_u64(...)` → receiver `x86`), but the module is registered under
//! its canonical dotted name (`arch.x86_64`). This module bridges the two: it
//! recognises an `arch.*` import path and builds the receiver-name → arch map
//! that sema and lowering both consult to resolve the alias. Keeping the logic
//! here — in `syntax`, a dependency of both `sema` and `ir` — keeps the two
//! phases in lock-step from a single source of truth.

use std::collections::HashMap;

use crate::ast::{CfgArch, Import, Item, Module};

/// The architecture an `arch.*` sub-module import path targets, or `None` when
/// `path` is not one of the recognised arch sub-modules. `arch.x86_64` →
/// [`CfgArch::X86_64`], `arch.aarch64` → [`CfgArch::Aarch64`]. The bare `arch`
/// facade (the portable `cpu_has` query, #963) is intentionally **not** an arch
/// sub-module — it compiles on every target — so it is not matched here.
pub fn arch_submodule(path: &str) -> Option<CfgArch> {
    match path {
        "arch.x86_64" => Some(CfgArch::X86_64),
        "arch.aarch64" => Some(CfgArch::Aarch64),
        _ => None,
    }
}

/// The canonical built-in module name an arch sub-module is registered under in
/// the sema / lowering / codegen module registries.
pub fn arch_module_name(arch: CfgArch) -> &'static str {
    match arch {
        CfgArch::X86_64 => "arch.x86_64",
        CfgArch::Aarch64 => "arch.aarch64",
    }
}

/// Build the receiver-name → arch map for every `import arch.x86_64 [as name]` /
/// `import arch.aarch64 [as name]` in `module`.
///
/// The key is the name the programmer calls the module by: the explicit `as`
/// alias when present, otherwise the import path's last segment (`arch.x86_64`
/// → `x86_64`). The value is the targeted arch, from which
/// [`arch_module_name`] gives the canonical registry name. A call
/// `x86.crc32c_u64(..)` is then resolved by looking up its receiver (`x86`) in
/// this map. Non-`arch` imports are ignored.
///
/// Resolution runs **after** `when target.*` cfg resolution ([`crate::cfg`]), so
/// an `import arch.x86_64` that lives in a dead cfg arm has already been removed
/// and never reaches this map — exactly how one source carries both arch paths.
pub fn collect_arch_imports(module: &Module) -> HashMap<String, CfgArch> {
    let mut map = HashMap::new();
    for item in &module.items {
        if let Item::Import(Import::Module { path, alias, .. }) = item {
            if let Some(arch) = arch_submodule(path) {
                let name = alias
                    .clone()
                    .unwrap_or_else(|| path.rsplit('.').next().unwrap_or(path).to_string());
                map.insert(name, arch);
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_str;

    #[test]
    fn recognises_arch_submodules() {
        assert_eq!(arch_submodule("arch.x86_64"), Some(CfgArch::X86_64));
        assert_eq!(arch_submodule("arch.aarch64"), Some(CfgArch::Aarch64));
        // The bare facade and unrelated modules are not arch sub-modules.
        assert_eq!(arch_submodule("arch"), None);
        assert_eq!(arch_submodule("arch.riscv"), None);
        assert_eq!(arch_submodule("math"), None);
    }

    #[test]
    fn canonical_names_round_trip() {
        assert_eq!(
            arch_submodule(arch_module_name(CfgArch::X86_64)),
            Some(CfgArch::X86_64)
        );
        assert_eq!(
            arch_submodule(arch_module_name(CfgArch::Aarch64)),
            Some(CfgArch::Aarch64)
        );
    }

    #[test]
    fn collects_aliased_and_bare_imports() {
        let src = "\
import arch.x86_64 as x86
import arch.aarch64

def main():
    pass
";
        let (module, perrs, lerrs) = parse_str(src);
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        let map = collect_arch_imports(&module);
        // Explicit alias wins; a bare import falls back to the last path segment.
        assert_eq!(map.get("x86"), Some(&CfgArch::X86_64));
        assert_eq!(map.get("aarch64"), Some(&CfgArch::Aarch64));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn ignores_non_arch_imports() {
        let src = "\
import math
import arch.x86_64 as x86

def main():
    pass
";
        let (module, _, _) = parse_str(src);
        let map = collect_arch_imports(&module);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("x86"));
    }
}
