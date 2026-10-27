//! Compile-time conditional compilation (`when target.*`, I#139 / D-0079).
//!
//! `when target.*:` is the low-level-perf escape hatch's **first** dependency
//! (design/0032 §4): it lets one source file carry per-architecture / per-OS /
//! per-feature code and have the compiler keep only the arm that matches the
//! build target. This is the mechanism that makes D-0079's "no silent fallback"
//! rule livable — an off-target `import arch.x86_64` is a hard error, so a
//! portable program needs a compile-time way to write both paths.
//!
//! The construct is resolved here, over the AST, **before** name resolution,
//! type-checking, and lowering. [`resolve_cfg`] walks the module, evaluates each
//! `when`/`elif`/`else` predicate against the active [`Target`], splices the
//! first matching arm's items/statements into the enclosing list, and discards
//! every other arm. Because the dead arms are removed here, the later phases
//! never see them — this is exactly the design's **dead-arm skip**: a dead arm
//! is *parsed* (so it must be syntactically valid) but is never name-resolved or
//! type-checked against a target it was not written for. A `when` block
//! introduces **no new scope**: the surviving arm's declarations land directly
//! in the enclosing module / block, so a `fn` defined inside an arm is visible
//! to the rest of the file.
//!
//! The predicate surface is the closed v1 set from §4: `target.arch == .<arch>`,
//! `target.os == .<os>`, and `target.has(.<feature>)` for a **compile-time**
//! feature (one baked into the build via `--target-feature`, distinct from the
//! later runtime `cpu.has`). Boolean combinators are intentionally not part of
//! v1.
//!
//! ## What "active target" means in Phase 1
//!
//! Zap builds natively for the host today (there is no cross-compiling
//! backend yet), so the default [`Target`] is the host the compiler binary runs
//! on — read from `std::env::consts::{ARCH, OS}`, with no LLVM dependency so
//! this stays usable in the host-only front end. A caller may override the
//! target (e.g. a `--target` / `--target-feature` flag, or a test) to evaluate
//! `when` for a different target; in Phase 1 that steers only this compile-time
//! selection and diagnostics, not actual code generation.

use crate::ast::{
    CfgArch, CfgItemBlock, CfgOs, CfgPredicate, CfgStmtBlock, DecoratorDef, EnumDef, Function,
    Item, Module, Stmt, StructDef,
};

/// The build target a `when target.*` block is evaluated against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub arch: Option<CfgArch>,
    pub os: Option<CfgOs>,
    /// Compile-time features baked into the build (`--target-feature`). A
    /// `target.has(.f)` predicate is true iff `f` is in this set.
    pub features: Vec<String>,
}

impl Target {
    /// A target with the given arch/os and no compile-time features.
    pub fn new(arch: Option<CfgArch>, os: Option<CfgOs>) -> Target {
        Target {
            arch,
            os,
            features: Vec::new(),
        }
    }

    /// The host the compiler is running on (`std::env::consts`). Unknown
    /// arch/OS values map to `None`, so a predicate naming them is simply never
    /// true — the build still succeeds via another arm or the `else`.
    pub fn host() -> Target {
        Target {
            arch: arch_from_str(std::env::consts::ARCH),
            os: os_from_str(std::env::consts::OS),
            features: Vec::new(),
        }
    }

    /// Does this target satisfy `pred`?
    pub fn satisfies(&self, pred: &CfgPredicate) -> bool {
        match pred {
            CfgPredicate::Arch(a) => self.arch == Some(*a),
            CfgPredicate::Os(o) => self.os == Some(*o),
            CfgPredicate::HasFeature(f) => self.features.iter().any(|have| have == f),
        }
    }
}

/// Map a Rust `std::env::consts::ARCH` (or `--target` arch token) to the closed
/// v1 arch set. Anything outside the set is `None` (never matches).
pub fn arch_from_str(s: &str) -> Option<CfgArch> {
    match s {
        "x86_64" => Some(CfgArch::X86_64),
        "aarch64" | "arm64" => Some(CfgArch::Aarch64),
        _ => None,
    }
}

/// Map a Rust `std::env::consts::OS` (or `--target` os token) to the closed v1
/// OS set. `macos`/`darwin` both map to [`CfgOs::Macos`]. Anything outside the
/// set is `None` (never matches).
pub fn os_from_str(s: &str) -> Option<CfgOs> {
    match s {
        "linux" => Some(CfgOs::Linux),
        "windows" => Some(CfgOs::Windows),
        "macos" | "darwin" => Some(CfgOs::Macos),
        _ => None,
    }
}

/// Resolve every `when target.*` block in `module` against `target`, in place.
///
/// After this runs the AST contains no [`Item::Cfg`] / [`Stmt::Cfg`] nodes: each
/// has been replaced by the body of its first matching arm (or its `else`, or
/// nothing). This is the single entry point the front end calls before sema and
/// lowering; it is idempotent (a second call is a no-op on an already-resolved
/// module).
pub fn resolve_cfg(module: &mut Module, target: &Target) {
    resolve_items(&mut module.items, target);
}

/// Pick the body of the first matching arm of an item-level `when` block (or its
/// `else`), already cfg-resolved, ready to splice in place.
fn select_item_body(block: &mut CfgItemBlock, target: &Target) -> Vec<Item> {
    for arm in &mut block.arms {
        if target.satisfies(&arm.predicate) {
            let mut body = std::mem::take(&mut arm.body);
            resolve_items(&mut body, target);
            return body;
        }
    }
    if let Some(body) = block.else_body.as_mut() {
        let mut body = std::mem::take(body);
        resolve_items(&mut body, target);
        return body;
    }
    Vec::new()
}

/// Pick the statements of the first matching arm of a statement-level `when`
/// block (or its `else`), already cfg-resolved, ready to splice in place.
fn select_stmt_body(block: &mut CfgStmtBlock, target: &Target) -> Vec<Stmt> {
    for arm in &mut block.arms {
        if target.satisfies(&arm.predicate) {
            let mut stmts = std::mem::take(&mut arm.body.stmts);
            resolve_stmts(&mut stmts, target);
            return stmts;
        }
    }
    if let Some(body) = block.else_body.as_mut() {
        let mut stmts = std::mem::take(&mut body.stmts);
        resolve_stmts(&mut stmts, target);
        return stmts;
    }
    Vec::new()
}

/// Resolve cfg in a list of items: expand each [`Item::Cfg`] in place and
/// descend into the bodies of the items that survive.
fn resolve_items(items: &mut Vec<Item>, target: &Target) {
    let mut out: Vec<Item> = Vec::with_capacity(items.len());
    for item in std::mem::take(items) {
        match item {
            Item::Cfg(mut block) => {
                let body = select_item_body(&mut block, target);
                out.extend(body);
            }
            mut other => {
                resolve_in_item(&mut other, target);
                out.push(other);
            }
        }
    }
    *items = out;
}

/// Descend into an item that is kept, resolving statement-level `when` in any
/// bodies it carries.
fn resolve_in_item(item: &mut Item, target: &Target) {
    match item {
        Item::Function(f) => resolve_in_function(f, target),
        Item::Struct(s) => resolve_in_struct(s, target),
        Item::Enum(e) => resolve_in_enum(e, target),
        Item::Decorator(d) => resolve_in_decorator(d, target),
        // A bare top-level statement: a top-level `when` is parsed as
        // [`Item::Cfg`], never as an `Item::Stmt`, so this is never a
        // `Stmt::Cfg` itself — only its nested blocks can hold one.
        Item::Stmt(s) => resolve_in_stmt(s, target),
        // No bodies to descend into.
        Item::Interface(_) | Item::Annotation(_) | Item::Import(_) => {}
        // Already expanded by `resolve_items`.
        Item::Cfg(_) => {}
    }
}

fn resolve_in_function(f: &mut Function, target: &Target) {
    resolve_stmts(&mut f.body.stmts, target);
}

fn resolve_in_struct(s: &mut StructDef, target: &Target) {
    for m in &mut s.methods {
        resolve_in_function(m, target);
    }
}

fn resolve_in_enum(e: &mut EnumDef, target: &Target) {
    for m in &mut e.methods {
        resolve_in_function(m, target);
    }
}

fn resolve_in_decorator(d: &mut DecoratorDef, target: &Target) {
    resolve_stmts(&mut d.body.stmts, target);
}

/// Resolve cfg in a list of statements: expand each [`Stmt::Cfg`] in place and
/// descend into the nested blocks of the statements that survive.
fn resolve_stmts(stmts: &mut Vec<Stmt>, target: &Target) {
    let mut out: Vec<Stmt> = Vec::with_capacity(stmts.len());
    for stmt in std::mem::take(stmts) {
        match stmt {
            Stmt::Cfg(mut block) => {
                let body = select_stmt_body(&mut block, target);
                out.extend(body);
            }
            mut other => {
                resolve_in_stmt(&mut other, target);
                out.push(other);
            }
        }
    }
    *stmts = out;
}

/// Descend into a statement that is kept, resolving `when` in its nested blocks.
fn resolve_in_stmt(stmt: &mut Stmt, target: &Target) {
    match stmt {
        Stmt::If {
            branches,
            else_block,
            ..
        } => {
            for (_cond, body) in branches.iter_mut() {
                resolve_stmts(&mut body.stmts, target);
            }
            if let Some(b) = else_block {
                resolve_stmts(&mut b.stmts, target);
            }
        }
        Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. }
        | Stmt::For { body, .. }
        | Stmt::Scope { body, .. }
        | Stmt::With { body, .. } => {
            resolve_stmts(&mut body.stmts, target);
        }
        Stmt::Match { arms, .. } => {
            for arm in arms.iter_mut() {
                resolve_stmts(&mut arm.body.stmts, target);
            }
        }
        Stmt::Try {
            body,
            excepts,
            finally,
            ..
        } => {
            resolve_stmts(&mut body.stmts, target);
            for ex in excepts.iter_mut() {
                resolve_stmts(&mut ex.body.stmts, target);
            }
            if let Some(f) = finally {
                resolve_stmts(&mut f.stmts, target);
            }
        }
        // Already expanded by `resolve_stmts`.
        Stmt::Cfg(_) => {}
        // Statements with no nested statement blocks.
        Stmt::Let { .. }
        | Stmt::Assign { .. }
        | Stmt::AugAssign { .. }
        | Stmt::Expr(_)
        | Stmt::Return { .. }
        | Stmt::Raise { .. }
        | Stmt::Assert { .. }
        | Stmt::Spawn { .. }
        | Stmt::LetTuple { .. }
        | Stmt::AssignTuple { .. }
        | Stmt::Break(_)
        | Stmt::Continue(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_str;

    fn x86() -> Target {
        Target::new(Some(CfgArch::X86_64), Some(CfgOs::Linux))
    }
    fn arm() -> Target {
        Target::new(Some(CfgArch::Aarch64), Some(CfgOs::Linux))
    }

    /// Resolve `src` for `target` and return the surviving top-level function
    /// names, in order.
    fn resolved_fn_names(src: &str, target: &Target) -> Vec<String> {
        let (mut module, perrs, lerrs) = parse_str(src);
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        resolve_cfg(&mut module, target);
        module
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Function(f) => Some(f.name.clone()),
                _ => None,
            })
            .collect()
    }

    const ARCH_SRC: &str = "\
when target.arch == .x86_64:
    def step() -> int64:
        return 1
elif target.arch == .aarch64:
    def step() -> int64:
        return 2
else:
    def step() -> int64:
        return 3

def main():
    print(step())
";

    #[test]
    fn selects_matching_arch_arm() {
        // Both targets keep exactly one `step` plus `main`.
        assert_eq!(resolved_fn_names(ARCH_SRC, &x86()), vec!["step", "main"]);
        assert_eq!(resolved_fn_names(ARCH_SRC, &arm()), vec!["step", "main"]);
    }

    #[test]
    fn falls_through_to_else() {
        let names = resolved_fn_names(
            ARCH_SRC,
            &Target::new(None, Some(CfgOs::Linux)), // unknown arch → else
        );
        assert_eq!(names, vec!["step", "main"]);
    }

    #[test]
    fn dead_arm_is_not_present_after_resolution() {
        // The x86 arm references a name only defined for aarch64; after
        // resolving for x86_64 the dead (aarch64) arm is gone, so nothing
        // downstream ever sees `aarch64_only`.
        let src = "\
when target.arch == .x86_64:
    def go() -> int64:
        return 10
elif target.arch == .aarch64:
    def go() -> int64:
        return aarch64_only()
    def aarch64_only() -> int64:
        return 20
";
        assert_eq!(resolved_fn_names(src, &x86()), vec!["go"]);
        // For aarch64 both go and its helper survive.
        assert_eq!(resolved_fn_names(src, &arm()), vec!["go", "aarch64_only"]);
    }

    #[test]
    fn os_predicate() {
        let src = "\
when target.os == .windows:
    def sep() -> str:
        return \"\\\\\"
else:
    def sep() -> str:
        return \"/\"
";
        // On a non-Windows target only the `else` survives.
        assert_eq!(
            resolved_fn_names(src, &x86()),
            vec!["sep"],
            "exactly one sep survives"
        );
    }

    #[test]
    fn has_feature_predicate() {
        let src = "\
when target.has(.sse4_2):
    def fast() -> int64:
        return 1
else:
    def fast() -> int64:
        return 0
";
        let mut with_feat = x86();
        with_feat.features.push("sse4_2".to_string());
        assert_eq!(resolved_fn_names(src, &with_feat), vec!["fast"]);
        // Without the feature baked in, the `else` arm is taken (still one fn).
        assert_eq!(resolved_fn_names(src, &x86()), vec!["fast"]);
    }

    #[test]
    fn statement_level_when() {
        let src = "\
def pick() -> int64:
    when target.arch == .x86_64:
        auto v = 1
    elif target.arch == .aarch64:
        auto v = 2
    else:
        auto v = 3
    return v
";
        let (mut module, perrs, lerrs) = parse_str(src);
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        resolve_cfg(&mut module, &x86());
        // The function body has the `when` expanded to the single `auto v = ...`
        // declaration plus the `return`, i.e. two statements, no `Stmt::Cfg`.
        let Item::Function(f) = &module.items[0] else {
            panic!("expected a function");
        };
        assert_eq!(f.body.stmts.len(), 2);
        assert!(!matches!(f.body.stmts[0], Stmt::Cfg(_)));
    }

    #[test]
    fn no_matching_arm_and_no_else_drops_the_block() {
        let src = "\
when target.os == .windows:
    def only_windows():
        pass

def main():
    pass
";
        // Non-Windows target with no else: the block vanishes entirely.
        assert_eq!(resolved_fn_names(src, &x86()), vec!["main"]);
    }

    #[test]
    fn host_target_is_in_the_closed_set_or_none() {
        // Sanity: the host maps cleanly (the CI hosts are x86_64/aarch64 Linux).
        let t = Target::host();
        // Just assert it does not panic and features start empty.
        assert!(t.features.is_empty());
        let _ = (t.arch, t.os);
    }
}
