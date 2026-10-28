//! Zap-to-Zap module & package imports — the driver's module linker
//! (I#283, D-0081).
//!
//! Today the compiler reads a single entry file. This module turns the entry
//! into a *program*: it discovers the local import graph, parses each module
//! once, enforces the module rules (declaration-only non-entry modules,
//! `_`-privacy, no import cycles), and **merges** the whole graph into one AST
//! [`Module`] with module-qualified symbols — so the existing single-module
//! lower → codegen → link path (and the AOT single-LLVM-module invariant, which
//! keeps the D-0080 REPL/JIT internal-linkage behaviour out of scope) is reused
//! unchanged.
//!
//! Stage A (this cut) resolves **intra-project local modules** only: a module
//! `a` resolves to `<root>/a.zap` and `a.b` to `<root>/a/b.zap`, where `<root>`
//! is the entry file's own directory. Cross-package (dependency) imports — the
//! leading segment being a dependency logical name resolved through `zenv` — are
//! Stage B and reuse this same loader (only root selection differs).
//!
//! Resolution precedence (D-0081): a leading segment that names a compiler-known
//! stdlib module (`math`, `os`, …) is left to the existing built-in handling and
//! never resolved to a file; `import python` is untouched.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use zapc_diag::ErrorCode;
use zapc_syntax::ast::{Expr, Import, Item, Module, Stmt};
use zapc_syntax::cfg::Target;
use zapc_syntax::span::Span;

mod rewrite;

/// A diagnostic raised by the module linker, carrying the source file it refers
/// to so multi-file programs report precise locations.
#[derive(Debug)]
pub struct ModDiag {
    pub code: ErrorCode,
    pub message: String,
    pub span: Span,
    pub file: String,
}

/// The mangle prefix for a module's top-level symbols. The entry module keeps
/// its original names (so `main` stays `main` and the single-file path is byte-
/// identical); every imported module gets a reserved, collision-free prefix.
fn mangle_prefix(sanitized: &str, is_entry: bool) -> String {
    if is_entry {
        String::new()
    } else {
        format!("__zm_{sanitized}__")
    }
}

/// Turn a dotted module path into a prefix-safe token (`a.b` → `a_b`).
fn sanitize(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// A module after parsing + cfg resolution, before merging.
struct SrcModule {
    /// Dotted module path (`a`, `a.b`); `<entry>` for the entry module.
    path: String,
    /// Mangle prefix for this module's top-level symbols.
    prefix: String,
    file: String,
    ast: Module,
    is_entry: bool,
    /// `bound name -> module path` for each resolved local `import` in this file.
    imports: Vec<ResolvedImport>,
}

struct ResolvedImport {
    /// The name this import binds in the importing module (alias, else the
    /// module path's last segment).
    bound: String,
    /// The target module's dotted path.
    target_path: String,
    span: Span,
}

/// The public view of a target module needed to resolve a qualified access.
struct ModuleFacts {
    prefix: String,
    /// All top-level symbol names this module declares (public and private).
    symbols: HashSet<String>,
}

/// Load the program rooted at `entry_path`: discover + parse the local import
/// graph, enforce the module rules, and return the single merged, qualified
/// [`Module`]. On any linker diagnostic, prints them and returns the diagnostics
/// so the caller can fail the build.
pub fn load_program(entry_path: &str, target: &Target) -> Result<Module, Vec<ModDiag>> {
    let mut loader = Loader::new(target);
    let merged = loader.run(entry_path);
    if loader.diags.is_empty() {
        merged.ok_or_else(Vec::new)
    } else {
        Err(loader.diags)
    }
}

struct Loader<'t> {
    target: &'t Target,
    /// Project source root — the entry file's directory. All Stage-A local
    /// imports resolve under it.
    root: PathBuf,
    /// Canonical file path -> loaded module index, so each file loads once.
    loaded: HashMap<PathBuf, usize>,
    modules: Vec<SrcModule>,
    diags: Vec<ModDiag>,
}

impl<'t> Loader<'t> {
    fn new(target: &'t Target) -> Self {
        Loader {
            target,
            root: PathBuf::new(),
            loaded: HashMap::new(),
            modules: Vec::new(),
            diags: Vec::new(),
        }
    }

    fn run(&mut self, entry_path: &str) -> Option<Module> {
        let entry = PathBuf::from(entry_path);
        self.root = entry.parent().map(Path::to_path_buf).unwrap_or_default();
        // Load the entry and, transitively, every local module it imports.
        let entry_idx = self.load_file(&entry, "<entry>", true)?;
        let mut queue = vec![entry_idx];
        while let Some(idx) = queue.pop() {
            // Clone the small import list to avoid holding a borrow of `self`.
            let imports: Vec<(String, Span)> = self.modules[idx]
                .imports
                .iter()
                .map(|i| (i.target_path.clone(), i.span))
                .collect();
            let file = self.modules[idx].file.clone();
            for (path, span) in imports {
                let Some(target_file) = self.resolve_module_file(&path) else {
                    self.diag(
                        ErrorCode::ZAP3010,
                        format!(
                            "cannot find module `{path}` (looked for `{}`)",
                            self.expected_rel(&path)
                        ),
                        span,
                        &file,
                    );
                    continue;
                };
                let canon = canonical(&target_file);
                if self.loaded.contains_key(&canon) {
                    continue;
                }
                if let Some(new_idx) = self.load_file(&target_file, &path, false) {
                    queue.push(new_idx);
                }
            }
        }
        if self.diags.is_empty() {
            self.detect_cycles();
        }
        if self.diags.is_empty() {
            self.check_declaration_only();
        }
        if !self.diags.is_empty() {
            return None;
        }
        Some(self.merge())
    }

    /// The path we expected a module to live at, for the "not found" message.
    fn expected_rel(&self, path: &str) -> String {
        let rel: PathBuf = path.split('.').collect::<PathBuf>();
        self.root
            .join(rel)
            .with_extension("zap")
            .display()
            .to_string()
    }

    /// Resolve a dotted module path to a file under the project root, or `None`
    /// if no such file exists. Stage A: local only.
    fn resolve_module_file(&self, path: &str) -> Option<PathBuf> {
        let rel: PathBuf = path.split('.').collect::<PathBuf>();
        let file = self.root.join(rel).with_extension("zap");
        file.exists().then_some(file)
    }

    /// Parse + cfg-resolve a file, record it, and return its module index.
    /// Lex/parse errors are collected and reported.
    fn load_file(&mut self, file: &Path, path: &str, is_entry: bool) -> Option<usize> {
        let canon = canonical(file);
        if let Some(&idx) = self.loaded.get(&canon) {
            return Some(idx);
        }
        let file_label = file.display().to_string();
        let src = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                self.diag(
                    ErrorCode::ZAP3010,
                    format!("cannot read module `{path}`: {e}"),
                    Span::new(0, 0),
                    &file_label,
                );
                return None;
            }
        };
        let (mut ast, perrs, lerrs) = zapc_syntax::parser::parse_str(&src);
        zapc_syntax::cfg::resolve_cfg(&mut ast, self.target);
        let mut had_err = false;
        for e in &lerrs {
            self.diag(e.code, e.message.clone(), e.span, &file_label);
            had_err = true;
        }
        for e in &perrs {
            self.diag(e.code, e.message.clone(), e.span, &file_label);
            had_err = true;
        }
        if had_err {
            return None;
        }
        let prefix = mangle_prefix(&sanitize(path), is_entry);
        let imports = collect_imports(&ast);
        let idx = self.modules.len();
        self.modules.push(SrcModule {
            path: path.to_string(),
            prefix,
            file: file_label,
            ast,
            is_entry,
            imports,
        });
        self.loaded.insert(canon, idx);
        Some(idx)
    }

    /// Reject import cycles (D-0081): a DFS back-edge over the resolved graph.
    /// The entry is index 0; every module is reachable from it by construction.
    fn detect_cycles(&mut self) {
        let n = self.modules.len();
        // Precompute each module's out-edges (as module indices).
        let by_path: HashMap<&str, usize> = self
            .modules
            .iter()
            .enumerate()
            .map(|(i, m)| (m.path.as_str(), i))
            .collect();
        let edges: Vec<Vec<usize>> = self
            .modules
            .iter()
            .map(|m| {
                m.imports
                    .iter()
                    .filter_map(|i| by_path.get(i.target_path.as_str()).copied())
                    .collect()
            })
            .collect();
        // 0 = white (unvisited), 1 = gray (on the DFS stack), 2 = black (done).
        let mut color = vec![0u8; n];
        let mut path: Vec<usize> = Vec::new();
        for start in 0..n {
            if color[start] == 0 {
                if let Some(cycle) = dfs_cycle(start, &edges, &mut color, &mut path) {
                    let names: Vec<String> = cycle
                        .iter()
                        .map(|&i| self.modules[i].display_name())
                        .collect();
                    // Attribute the diagnostic to the import edge that closed
                    // the cycle (the last node importing the first).
                    let last = *cycle.last().unwrap();
                    let first = cycle[0];
                    let (span, file) = self.modules[last]
                        .imports
                        .iter()
                        .find(|imp| by_path.get(imp.target_path.as_str()) == Some(&first))
                        .map(|imp| (imp.span, self.modules[last].file.clone()))
                        .unwrap_or((Span::new(0, 0), self.modules[last].file.clone()));
                    self.diag(
                        ErrorCode::ZAP3011,
                        format!(
                            "circular import: {} -> {}",
                            names.join(" -> "),
                            self.modules[first].display_name()
                        ),
                        span,
                        &file,
                    );
                    return;
                }
            }
        }
    }

    /// Non-entry modules are declaration-only (D-0081): a top-level executable
    /// statement is an error.
    fn check_declaration_only(&mut self) {
        let mut found = Vec::new();
        for m in &self.modules {
            if m.is_entry {
                continue;
            }
            for item in &m.ast.items {
                if let Item::Stmt(st) = item {
                    found.push((stmt_span(st), m.file.clone()));
                }
            }
        }
        for (span, file) in found {
            self.diag(
                ErrorCode::ZAP3014,
                "top-level statements are not allowed in an imported module \
                 (only the entry module runs top-level code)"
                    .to_string(),
                span,
                &file,
            );
        }
    }

    /// Qualify every module's symbols and references, then concatenate into one
    /// AST module.
    fn merge(&mut self) -> Module {
        // Facts about every module, keyed by dotted path, for cross-module
        // qualified-access resolution.
        let facts: HashMap<String, ModuleFacts> = self
            .modules
            .iter()
            .map(|m| {
                (
                    m.path.clone(),
                    ModuleFacts {
                        prefix: m.prefix.clone(),
                        symbols: top_level_symbols(&m.ast),
                    },
                )
            })
            .collect();

        let mut items: Vec<Item> = Vec::new();
        // Collect diagnostics from the per-module rewrite without borrowing
        // `self.diags` across the module loop.
        let mut rw_diags: Vec<ModDiag> = Vec::new();
        for m in &self.modules {
            // `alias -> target facts` for this module's imports.
            let aliases: HashMap<String, &ModuleFacts> = m
                .imports
                .iter()
                .filter_map(|i| facts.get(&i.target_path).map(|f| (i.bound.clone(), f)))
                .collect();
            let own = top_level_symbols(&m.ast);
            let mut rw = rewrite::Rewriter {
                prefix: &m.prefix,
                own: &own,
                aliases: &aliases,
                file: &m.file,
                diags: &mut rw_diags,
            };
            for item in &m.ast.items {
                // Zap `import` statements are consumed by the linker and do not
                // survive into the merged module. `import python` is kept so
                // sema still binds its aliases (D-0012/D-0028); stdlib
                // `import <name>` is also a Zap `Import::Module` and is dropped
                // (stdlib member access resolves by the literal base name, so
                // the import line is not load-bearing — I#70/I#121).
                if matches!(item, Item::Import(Import::Module { .. })) {
                    continue;
                }
                let mut it = item.clone();
                rw.item(&mut it);
                items.push(it);
            }
        }
        self.diags.extend(rw_diags);
        let span = self
            .modules
            .first()
            .map(|m| m.ast.span)
            .unwrap_or(Span::new(0, 0));
        Module { items, span }
    }

    fn diag(&mut self, code: ErrorCode, message: String, span: Span, file: &str) {
        self.diags.push(ModDiag {
            code,
            message,
            span,
            file: file.to_string(),
        });
    }
}

impl SrcModule {
    fn display_name(&self) -> String {
        if self.is_entry {
            format!("{} (entry)", self.path)
        } else {
            self.path.clone()
        }
    }
}

/// Resolve an import list of a parsed module into bound-name/target-path pairs,
/// skipping `import python` and stdlib modules (left to the built-in handling).
fn collect_imports(ast: &Module) -> Vec<ResolvedImport> {
    let mut out = Vec::new();
    for item in &ast.items {
        if let Item::Import(Import::Module { path, alias, span }) = item {
            let first = path.split('.').next().unwrap_or(path);
            if zapc_sema::is_builtin_module(first) {
                continue;
            }
            let bound = alias
                .clone()
                .unwrap_or_else(|| path.rsplit('.').next().unwrap_or(path).to_string());
            out.push(ResolvedImport {
                bound,
                target_path: path.clone(),
                span: *span,
            });
        }
    }
    out
}

/// The set of top-level symbol names a module declares that participate in
/// qualified cross-module access and mangling (functions, structs, enums,
/// interfaces). Annotation/decorator declarations are intentionally excluded in
/// Stage A.
fn top_level_symbols(ast: &Module) -> HashSet<String> {
    let mut s = HashSet::new();
    for item in &ast.items {
        match item {
            Item::Function(f) => {
                s.insert(f.name.clone());
            }
            Item::Struct(d) => {
                s.insert(d.name.clone());
            }
            Item::Enum(e) => {
                s.insert(e.name.clone());
            }
            Item::Interface(i) => {
                s.insert(i.name.clone());
            }
            _ => {}
        }
    }
    s
}

/// Recursive DFS for cycle detection. Returns the nodes of a cycle (the gray
/// node first, through to the node whose edge closed it) on the first back-edge,
/// else `None`.
fn dfs_cycle(
    node: usize,
    edges: &[Vec<usize>],
    color: &mut [u8],
    path: &mut Vec<usize>,
) -> Option<Vec<usize>> {
    color[node] = 1; // gray
    path.push(node);
    for &next in &edges[node] {
        match color[next] {
            1 => {
                // Back-edge: the cycle is path[pos(next)..].
                let pos = path.iter().position(|&n| n == next).unwrap();
                return Some(path[pos..].to_vec());
            }
            0 => {
                if let Some(c) = dfs_cycle(next, edges, color, path) {
                    return Some(c);
                }
            }
            _ => {}
        }
    }
    path.pop();
    color[node] = 2; // black
    None
}

/// Canonicalize a path for load-once identity, falling back to the lexical path
/// when the file cannot be canonicalized.
fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The source span of a statement (for the declaration-only diagnostic).
fn stmt_span(st: &Stmt) -> Span {
    match st {
        Stmt::Let { span, .. }
        | Stmt::LetTuple { span, .. }
        | Stmt::Assign { span, .. }
        | Stmt::AugAssign { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. }
        | Stmt::DoWhile { span, .. }
        | Stmt::For { span, .. }
        | Stmt::Match { span, .. }
        | Stmt::Return { span, .. }
        | Stmt::Raise { span, .. }
        | Stmt::Assert { span, .. }
        | Stmt::Try { span, .. }
        | Stmt::With { span, .. }
        | Stmt::Spawn { span, .. }
        | Stmt::AssignTuple { span, .. }
        | Stmt::Scope { span, .. } => *span,
        Stmt::Break(s) | Stmt::Continue(s) => *s,
        Stmt::Expr(e) => expr_span(e),
        Stmt::Cfg(b) => b.span,
    }
}

fn expr_span(e: &Expr) -> Span {
    use Expr::*;
    match e {
        Int(_, s) | Float(_, s) | Imaginary(_, s) | Bool(_, s) | None(s) | Name(_, s) => *s,
        Str { span, .. }
        | Tuple(_, span)
        | List(_, span)
        | Dict(_, span)
        | Set(_, span)
        | ListComp { span, .. }
        | DictComp { span, .. }
        | SetComp { span, .. }
        | Unary { span, .. }
        | Binary { span, .. }
        | Call { span, .. }
        | Index { span, .. }
        | Slice { span, .. }
        | Attr { span, .. }
        | Lambda { span, .. }
        | RegexMatch { span, .. }
        | RegexSubst { span, .. }
        | ChanRecv { span, .. }
        | Yield { span, .. }
        | IfElse { span, .. } => *span,
    }
}

#[cfg(test)]
mod tests;
