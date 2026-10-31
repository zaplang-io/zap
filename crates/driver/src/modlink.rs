//! Zap-to-Zap module & package imports — the driver's module linker
//! (I#283, D-0081).
//!
//! Today the compiler reads a single entry file. This module turns the entry
//! into a *program*: it discovers the import graph, parses each module once,
//! enforces the module rules (declaration-only non-entry modules, `_`-privacy,
//! no import cycles), and **merges** the whole graph into one AST [`Module`]
//! with module-qualified symbols — so the existing single-module
//! lower → codegen → link path (and the AOT single-LLVM-module invariant, which
//! keeps the D-0080 REPL/JIT internal-linkage behaviour out of scope) is reused
//! unchanged.
//!
//! Resolution (D-0081), by the import path's leading segment, in precedence
//! order:
//!   1. a compiler-known **stdlib** module (`math`, `os`, …) — left to the
//!      existing built-in handling, never resolved to a file; `import python`
//!      is untouched.
//!   2. a **dependency** logical name — resolved through `zenv`: the package
//!      lives at `<pkg_root>/<name>` and the module is `<pkg_root>/<name>/src/…`
//!      (Stage B); the remaining path segments map inside the package's `src/`,
//!      and a bare `import <name>` selects `src/<name>.zap`.
//!   3. a **local** module — resolved relative to the *importing* file's own
//!      package source root: `a` → `<root>/a.zap`, `a.b` → `<root>/a/b.zap`
//!      (Stage A). For the entry file `<root>` is its own directory.
//!
//! Module identity is per loaded file (not per dotted path), so two packages may
//! each contain a module of the same path without colliding; mangle prefixes are
//! keyed by the loaded-module index.

use std::collections::HashMap;
use std::collections::HashSet;
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

/// A module after parsing + cfg resolution, before merging.
struct SrcModule {
    /// Dotted module path as written at the import that first loaded it
    /// (`a`, `a.b`, a dependency's `<name>…`); `<entry>` for the entry module.
    path: String,
    /// Mangle prefix for this module's top-level symbols (empty for the entry).
    prefix: String,
    file: String,
    /// The package source root this module lives in — the directory its own
    /// sibling (`local`) imports resolve against. The entry's is its directory;
    /// a dependency module's is `<pkg_root>/<name>/src`.
    src_root: PathBuf,
    ast: Module,
    is_entry: bool,
}

/// One resolved `import` edge: the name it binds and the loaded-module index it
/// points at.
struct Edge {
    bound: String,
    target: usize,
    span: Span,
}

/// A raw (unresolved) import scanned from a parsed module.
struct RawImport {
    bound: String,
    path: String,
    span: Span,
}

/// The public view of a target module needed to resolve a qualified access.
struct ModuleFacts {
    prefix: String,
    /// All top-level symbol names this module declares (public and private).
    symbols: HashSet<String>,
}

/// Load the program rooted at `entry_path`, resolving dependency imports through
/// the active `zenv` search paths (D-0052). Returns the single merged, qualified
/// [`Module`], or the linker diagnostics.
pub fn load_program(entry_path: &str, target: &Target) -> Result<Module, Vec<ModDiag>> {
    let pkg_roots = zapc_pkg::zenv::module_search_paths_from_env();
    load_program_with(entry_path, target, pkg_roots)
}

/// Like [`load_program`], but with explicit dependency package-search roots (each
/// a directory containing `<name>/src/…`). Used by tests to avoid touching the
/// process environment.
pub fn load_program_with(
    entry_path: &str,
    target: &Target,
    pkg_roots: Vec<PathBuf>,
) -> Result<Module, Vec<ModDiag>> {
    let mut loader = Loader::new(target, pkg_roots);
    let merged = loader.run(entry_path);
    if loader.diags.is_empty() {
        merged.ok_or_else(Vec::new)
    } else {
        Err(loader.diags)
    }
}

struct Loader<'t> {
    target: &'t Target,
    /// Dependency package-search roots (`$ZAP_ZENV/pkg`, …): a dependency
    /// `<name>` is the directory `<root>/<name>`.
    pkg_roots: Vec<PathBuf>,
    /// Canonical file path -> loaded module index, so each file loads once.
    loaded: HashMap<PathBuf, usize>,
    modules: Vec<SrcModule>,
    /// Raw imports scanned from each module, parallel to `modules`.
    raw: Vec<Vec<RawImport>>,
    /// Resolved import edges per module, parallel to `modules`.
    edges: Vec<Vec<Edge>>,
    diags: Vec<ModDiag>,
}

impl<'t> Loader<'t> {
    fn new(target: &'t Target, pkg_roots: Vec<PathBuf>) -> Self {
        Loader {
            target,
            pkg_roots,
            loaded: HashMap::new(),
            modules: Vec::new(),
            raw: Vec::new(),
            edges: Vec::new(),
            diags: Vec::new(),
        }
    }

    fn run(&mut self, entry_path: &str) -> Option<Module> {
        let entry = PathBuf::from(entry_path);
        let root = entry.parent().map(Path::to_path_buf).unwrap_or_default();
        let entry_idx = self.load_file(&entry, "<entry>", root, true)?;
        // BFS the import graph, resolving each edge to a loaded-module index.
        let mut queue = vec![entry_idx];
        while let Some(idx) = queue.pop() {
            let importer_root = self.modules[idx].src_root.clone();
            let raw: Vec<(String, String, Span)> = self.raw[idx]
                .iter()
                .map(|r| (r.bound.clone(), r.path.clone(), r.span))
                .collect();
            let file = self.modules[idx].file.clone();
            for (bound, path, span) in raw {
                let Some((target_file, dep_src_root)) =
                    self.resolve_import(&importer_root, &path, span, &file)
                else {
                    continue; // diagnostic already pushed (or stdlib: skipped)
                };
                let canon = canonical(&target_file);
                let tidx = match self.loaded.get(&canon) {
                    Some(&i) => i,
                    None => {
                        let root = dep_src_root.unwrap_or_else(|| importer_root.clone());
                        match self.load_file(&target_file, &path, root, false) {
                            Some(i) => {
                                queue.push(i);
                                i
                            }
                            None => continue,
                        }
                    }
                };
                self.edges[idx].push(Edge {
                    bound,
                    target: tidx,
                    span,
                });
            }
        }
        if self.diags.is_empty() {
            self.detect_cycles();
        }
        if self.diags.is_empty() {
            self.check_toplevel_statements();
        }
        if !self.diags.is_empty() {
            return None;
        }
        Some(self.merge())
    }

    /// Resolve one import path from a module whose own package source root is
    /// `importer_root`. Returns the target file and, when it is a dependency
    /// module, that dependency's own source root. `None` means either a stdlib
    /// import (skipped, no diagnostic) or an unresolved one (diagnostic pushed).
    fn resolve_import(
        &mut self,
        importer_root: &Path,
        path: &str,
        span: Span,
        file: &str,
    ) -> Option<(PathBuf, Option<PathBuf>)> {
        let first = path.split('.').next().unwrap_or(path);
        // (1) stdlib — left to the built-in handling.
        if zapc_sema::is_builtin_module(first) {
            return None;
        }
        // (2) dependency package (D-0052): `<pkg_root>/<first>` is a directory.
        for pkg_root in &self.pkg_roots {
            let pkg_dir = pkg_root.join(first);
            if !pkg_dir.is_dir() {
                continue;
            }
            let dep_src = pkg_dir.join("src");
            // The remaining segments map inside the package's `src/`; a bare
            // `import <name>` selects `src/<name>.zap`.
            let rest = path.strip_prefix(first).unwrap_or("");
            let rest = rest.strip_prefix('.').unwrap_or(rest);
            let within = if rest.is_empty() { first } else { rest };
            let rel: PathBuf = within.split('.').collect();
            let f = dep_src.join(rel).with_extension("zap");
            if f.exists() {
                return Some((f, Some(dep_src)));
            }
            self.diag(
                ErrorCode::ZAP3010,
                format!(
                    "cannot find module `{path}` in dependency `{first}` (looked for `{}`)",
                    f.display()
                ),
                span,
                file,
            );
            return None;
        }
        // (3) local module relative to the importing file's package root.
        let rel: PathBuf = path.split('.').collect();
        let f = importer_root.join(&rel).with_extension("zap");
        if f.exists() {
            return Some((f, None));
        }
        self.diag(
            ErrorCode::ZAP3010,
            format!("cannot find module `{path}` (looked for `{}`)", f.display()),
            span,
            file,
        );
        None
    }

    /// Parse + cfg-resolve a file, record it (with its package source root), and
    /// return its module index. Lex/parse errors are collected and reported.
    fn load_file(
        &mut self,
        file: &Path,
        path: &str,
        src_root: PathBuf,
        is_entry: bool,
    ) -> Option<usize> {
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
        let idx = self.modules.len();
        // Unique, collision-free prefix per loaded module (entry keeps names).
        let prefix = if is_entry {
            String::new()
        } else {
            format!("__zm{idx}_{}__", sanitize(path))
        };
        let raw = collect_imports(&ast);
        self.modules.push(SrcModule {
            path: path.to_string(),
            prefix,
            file: file_label,
            src_root,
            ast,
            is_entry,
        });
        self.raw.push(raw);
        self.edges.push(Vec::new());
        self.loaded.insert(canon, idx);
        Some(idx)
    }

    /// Reject import cycles (D-0081): a DFS back-edge over the resolved graph.
    fn detect_cycles(&mut self) {
        let n = self.modules.len();
        let adj: Vec<Vec<usize>> = self
            .edges
            .iter()
            .map(|es| es.iter().map(|e| e.target).collect())
            .collect();
        let mut color = vec![0u8; n]; // 0 white, 1 gray, 2 black
        let mut path: Vec<usize> = Vec::new();
        for start in 0..n {
            if color[start] == 0 {
                if let Some(cycle) = dfs_cycle(start, &adj, &mut color, &mut path) {
                    let names: Vec<String> = cycle
                        .iter()
                        .map(|&i| self.modules[i].display_name())
                        .collect();
                    let last = *cycle.last().unwrap();
                    let first = cycle[0];
                    let (span, file) = self.edges[last]
                        .iter()
                        .find(|e| e.target == first)
                        .map(|e| (e.span, self.modules[last].file.clone()))
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

    /// Top-level executable statements are rejected in every module (D-0081,
    /// ticket #459). A compiled Zap program runs from `def main()`; a bare
    /// statement at module scope never lowers to code (the IR carries no
    /// top-level landing spot for one — it is silently dropped), so it is an
    /// error rather than a no-op, in the entry module (ZAP3016) and imported
    /// modules alike (ZAP3014, which also keeps them declaration-only). The one
    /// exception is a **module docstring** (D-0041): a bare, non-f-string string
    /// literal carries no behaviour, so it is allowed at module scope.
    fn check_toplevel_statements(&mut self) {
        let mut found = Vec::new();
        for m in &self.modules {
            for item in &m.ast.items {
                if let Item::Stmt(st) = item {
                    if is_doc_stmt(st) {
                        continue;
                    }
                    let code = if m.is_entry {
                        ErrorCode::ZAP3016
                    } else {
                        ErrorCode::ZAP3014
                    };
                    found.push((code, stmt_span(st), m.file.clone()));
                }
            }
        }
        for (code, span, file) in found {
            let message = if code == ErrorCode::ZAP3016 {
                "top-level executable statement: a compiled program runs from \
                 `def main()`, so this would never run — move it (and any \
                 `@receiver.method(...)` decorator on a `def`) into `def main()`"
                    .to_string()
            } else {
                "top-level statements are not allowed in an imported module \
                 (only declarations and a leading docstring); move executable \
                 code into a function the importer calls"
                    .to_string()
            };
            self.diag(code, message, span, &file);
        }
    }

    /// Qualify every module's symbols and references, then concatenate into one
    /// AST module.
    fn merge(&mut self) -> Module {
        let facts: Vec<ModuleFacts> = self
            .modules
            .iter()
            .map(|m| ModuleFacts {
                prefix: m.prefix.clone(),
                symbols: top_level_symbols(&m.ast),
            })
            .collect();

        let mut items: Vec<Item> = Vec::new();
        let mut rw_diags: Vec<ModDiag> = Vec::new();
        for (idx, m) in self.modules.iter().enumerate() {
            let aliases: HashMap<String, &ModuleFacts> = self.edges[idx]
                .iter()
                .map(|e| (e.bound.clone(), &facts[e.target]))
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

/// Turn a dotted module path into a prefix-safe token (`a.b` → `a_b`).
fn sanitize(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Recursive DFS for cycle detection. Returns the nodes of a cycle (the gray
/// node first, through to the node whose edge closed it) on the first back-edge,
/// else `None`.
fn dfs_cycle(
    node: usize,
    adj: &[Vec<usize>],
    color: &mut [u8],
    path: &mut Vec<usize>,
) -> Option<Vec<usize>> {
    color[node] = 1;
    path.push(node);
    for &next in &adj[node] {
        match color[next] {
            1 => {
                let pos = path.iter().position(|&n| n == next).unwrap();
                return Some(path[pos..].to_vec());
            }
            0 => {
                if let Some(c) = dfs_cycle(next, adj, color, path) {
                    return Some(c);
                }
            }
            _ => {}
        }
    }
    path.pop();
    color[node] = 2;
    None
}

/// Scan a parsed module's `import` statements into raw (bound-name, path) pairs,
/// skipping `import python`. Stdlib vs dependency vs local is decided later, at
/// resolution time, since it depends on the importing module's package root.
fn collect_imports(ast: &Module) -> Vec<RawImport> {
    let mut out = Vec::new();
    for item in &ast.items {
        if let Item::Import(Import::Module { path, alias, span }) = item {
            let bound = alias
                .clone()
                .unwrap_or_else(|| path.rsplit('.').next().unwrap_or(path).to_string());
            out.push(RawImport {
                bound,
                path: path.clone(),
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

/// Canonicalize a path for load-once identity, falling back to the lexical path
/// when the file cannot be canonicalized.
fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// A module docstring (D-0041): a bare, non-f-string string-literal statement.
/// Such a statement carries no runtime behaviour (it never lowers to code), so
/// it is allowed at module scope where an executable statement is not. An
/// f-string is *not* a docstring — its interpolations are executable — so it is
/// not exempted here.
fn is_doc_stmt(st: &Stmt) -> bool {
    matches!(st, Stmt::Expr(Expr::Str { fstring: false, .. }))
}

/// The source span of a statement (for the top-level-statement diagnostic).
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
        | Compare { span, .. }
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
