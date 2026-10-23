//! Queryable frontend analysis for Zap tooling (I#157/B1, issue #228).
//!
//! This crate is the foundation of the LSP (`design/0021-tooling.md` Area B,
//! `design/0022-analysis-api.md`). It runs the frontend — lex → parse →
//! type-check, no LLVM — over a single source document and exposes a
//! **position-keyed** view of the result that the compile path discards:
//!
//! - [`Analysis::diagnostics`] — structured diagnostics with stable `ZAPnnnn`
//!   codes (#239), the same payload the MCP `zap_check` surface serves.
//! - [`Analysis::type_at`] — the inferred type of the expression under a cursor.
//! - [`Analysis::symbol_at`] — the symbol (name, kind, type, definition) there.
//! - [`Analysis::definition_of`] — the go-to-definition target span.
//! - [`Analysis::references`] — in-document use sites of a symbol.
//! - [`Analysis::document_symbols`] — the outline tree (a pure AST walk, so it
//!   works even when the body has type errors).
//!
//! **How the resolver state is retained.** The type checker
//! ([`zapc_sema::analyze_queryable`]) records, as it already infers and
//! resolves, three span-keyed tables ([`zapc_sema::AnalysisFacts`]): the type of
//! every expression, every *local* use→definition link, and every binding's
//! definition site. Global (top-level) definitions and name uses are
//! AST-derivable, so this crate reconstructs those here rather than burdening
//! the checker. The compile path (`zapc_sema::check`/`analyze`) records nothing
//! and is unchanged — see the design note for the parity argument.
//!
//! **Positions.** A [`Pos`] is a raw byte offset into the source. [`LineIndex`]
//! maps between offsets and 1-based (line, column-in-bytes), matching the
//! existing `span_to_line_col`. The LSP's UTF-16 code-unit layer is #229's job
//! and sits on top of this without changing it.
//!
//! **Scope.** One document, recompute-per-snapshot (the #129 crate split makes
//! that cheap). The handle is immutable per snapshot, which is exactly the
//! boundary a future incremental/salsa engine would memoize behind, so #229 and
//! #230 are insulated from that choice. Cross-file resolution is a later
//! multi-module ticket (D-0052/#190).

use std::collections::HashMap;

use zapc_diag::ErrorCode;
use zapc_syntax::ast::*;
use zapc_syntax::span::Span;

pub use zapc_sema_types::Ty;

/// A position in the source, as a raw byte offset (D-0022 analysis API). Convert
/// to/from 1-based line/column with [`LineIndex`].
pub type Pos = u32;

/// Severity of a [`Diagnostic`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One diagnostic: its severity, producing phase, stable `ZAPnnnn` code, prose
/// message, raw span, and resolved 1-based line/column. Mirrors the MCP
/// `zap_check` payload so an LSP `publishDiagnostics` is a direct mapping.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    /// `"lex"`, `"parse"`, `"type"`, or `"warning"`.
    pub phase: &'static str,
    pub code: ErrorCode,
    pub message: String,
    pub span: Span,
    pub line: u32,
    pub col: u32,
}

/// What kind of thing a symbol is. Drives the LSP `SymbolKind` mapping (#230)
/// and lets a hover distinguish, say, a parameter from a top-level function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Param,
    Local,
    Struct,
    Enum,
    EnumVariant,
    Field,
    Interface,
    Annotation,
    Decorator,
    Import,
    Builtin,
}

/// The symbol under a cursor: its name, what kind of thing it is, its type (as
/// far as the checker inferred), and where it is defined.
#[derive(Clone, Debug)]
pub struct SymbolRef {
    pub name: String,
    pub kind: SymbolKind,
    pub ty: Ty,
    /// The definition site. For a local this is scope-accurate (from the
    /// checker); for a global it is the declaration span. `None` for a builtin
    /// or an unresolved name.
    pub def: Option<Span>,
}

/// A node in the document outline (LSP `documentSymbol`). `span` is the whole
/// declaration; `name_span` is the span to select/reveal (currently equal to
/// `span` for declarations with no name-only span — tightens when #225 lands).
#[derive(Clone, Debug)]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
    pub name_span: Span,
    /// A short signature/detail for display (e.g. a function's parameter list).
    pub detail: Option<String>,
    pub children: Vec<DocumentSymbol>,
}

/// Maps between byte offsets and 1-based (line, column-in-bytes). Column counts
/// bytes, matching the existing `span_to_line_col`; the LSP UTF-16 layer (#229)
/// is layered on top.
#[derive(Clone, Debug)]
pub struct LineIndex {
    /// Byte offset of the start of each line (`line_starts[0] == 0`).
    line_starts: Vec<u32>,
    len: u32,
}

impl LineIndex {
    pub fn new(src: &str) -> LineIndex {
        let mut line_starts = vec![0u32];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        LineIndex {
            line_starts,
            len: src.len() as u32,
        }
    }

    /// The 1-based (line, column) of a byte offset. Clamped to the source end.
    pub fn line_col(&self, pos: Pos) -> (u32, u32) {
        let pos = pos.min(self.len);
        // The line is the last line-start <= pos.
        let line = match self.line_starts.binary_search(&pos) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let col = pos - self.line_starts[line];
        (line as u32 + 1, col + 1)
    }

    /// The byte offset of a 1-based (line, column), or `None` if out of range.
    pub fn offset(&self, line: u32, col: u32) -> Option<Pos> {
        if line == 0 || col == 0 {
            return None;
        }
        let start = *self.line_starts.get((line - 1) as usize)?;
        let pos = start + (col - 1);
        if pos <= self.len {
            Some(pos)
        } else {
            None
        }
    }
}

/// Does `span` cover `pos`? Half-open `[start, end)`, except a zero-width span
/// matches only exactly at its position (so an empty span is still reachable).
fn covers(span: Span, pos: Pos) -> bool {
    if span.start == span.end {
        span.start == pos
    } else {
        pos >= span.start && pos < span.end
    }
}

/// The width of a span, used to pick the *innermost* (smallest) node covering a
/// position.
fn width(span: Span) -> u32 {
    span.end.saturating_sub(span.start)
}

/// A name occurrence found by the AST walk — either a bare name reference or the
/// member part of an attribute access.
#[derive(Clone, Debug)]
struct NameRef {
    name: String,
    /// The span of the name itself.
    span: Span,
    /// For `base.member`, the span of `base` (so its inferred type can resolve
    /// the member); `None` for a bare name.
    base_span: Option<Span>,
}

/// A top-level (global) definition the checker resolves by name rather than
/// through a scope: functions, types, imports.
#[derive(Clone, Debug)]
struct GlobalDef {
    span: Span,
    kind: SymbolKind,
}

/// A fully analysed single document (I#228). Build with [`Analysis::analyze`];
/// every query is a lookup against the retained tables.
pub struct Analysis {
    source: String,
    line_index: LineIndex,
    diagnostics: Vec<Diagnostic>,
    facts: zapc_sema::AnalysisFacts,
    doc_symbols: Vec<DocumentSymbol>,
    /// Top-level name → definition.
    global_defs: HashMap<String, GlobalDef>,
    /// `(type name, member name)` → member definition span (fields, methods,
    /// enum variants, interface methods) for attribute go-to-def.
    member_defs: HashMap<(String, String), Span>,
    /// Every bare-name reference in the document, for global `references`.
    name_refs: Vec<NameRef>,
}

impl Analysis {
    /// Lex, parse, type-check `source`, and retain the queryable result.
    pub fn analyze(source: &str) -> Analysis {
        let line_index = LineIndex::new(source);
        let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(source);

        let mut diagnostics = Vec::new();
        for e in &lerrs {
            diagnostics.push(make_diag(
                &line_index,
                Severity::Error,
                "lex",
                e.code,
                e.message.clone(),
                e.span,
            ));
        }
        for e in &perrs {
            diagnostics.push(make_diag(
                &line_index,
                Severity::Error,
                "parse",
                e.code,
                e.message.clone(),
                e.span,
            ));
        }

        // Only type-check a cleanly-parsed program (sema assumes a well-formed
        // AST), exactly as `check_source` does — so diagnostics match.
        let facts = if lerrs.is_empty() && perrs.is_empty() {
            let (errors, warnings, facts) = zapc_sema::analyze_queryable(&module);
            for e in &errors {
                diagnostics.push(make_diag(
                    &line_index,
                    Severity::Error,
                    "type",
                    e.code,
                    e.message.clone(),
                    e.span,
                ));
            }
            for w in &warnings {
                diagnostics.push(make_diag(
                    &line_index,
                    Severity::Warning,
                    "warning",
                    w.code,
                    w.message.clone(),
                    w.span,
                ));
            }
            facts
        } else {
            zapc_sema::AnalysisFacts::default()
        };

        diagnostics.sort_by_key(|d| (d.span.start, d.span.end));

        // The outline and global-name tables come from a pure AST walk over the
        // user module (no prelude), so they are available even on an errored
        // parse and never include synthetic prelude items.
        let mut walk = Walk::default();
        walk.module(&module);

        Analysis {
            source: source.to_string(),
            line_index,
            diagnostics,
            facts,
            doc_symbols: walk.symbols,
            global_defs: walk.global_defs,
            member_defs: walk.member_defs,
            name_refs: walk.name_refs,
        }
    }

    /// The source this analysis was built from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The position mapper for this document.
    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }

    /// Convenience: the byte offset of a 1-based (line, column).
    pub fn offset_at(&self, line: u32, col: u32) -> Option<Pos> {
        self.line_index.offset(line, col)
    }

    /// Convenience: the 1-based (line, column) of a byte offset.
    pub fn line_col(&self, pos: Pos) -> (u32, u32) {
        self.line_index.line_col(pos)
    }

    /// The document's diagnostics, in source order, with stable `ZAPnnnn` codes.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Whether the document type-checks (no error-severity diagnostics).
    pub fn ok(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// The document outline (functions, types, their fields/methods, …).
    pub fn document_symbols(&self) -> &[DocumentSymbol] {
        &self.doc_symbols
    }

    /// The inferred type of the innermost expression covering `pos`.
    pub fn type_at(&self, pos: Pos) -> Option<Ty> {
        self.facts
            .expr_types
            .iter()
            .filter(|(span, _)| covers(*span, pos))
            .min_by_key(|(span, _)| width(*span))
            .map(|(_, ty)| ty.clone())
    }

    /// The inferred type of the expression whose span is exactly `span` (the
    /// last such, so an outer node wins over an inner one that happens to share
    /// an offset); falls back to the innermost covering `span.start`.
    fn type_of_span(&self, span: Span) -> Option<Ty> {
        self.facts
            .expr_types
            .iter()
            .rev()
            .find(|(s, _)| s.start == span.start && s.end == span.end)
            .map(|(_, ty)| ty.clone())
            .or_else(|| self.type_at(span.start))
    }

    /// The symbol under the cursor: its name, kind, type, and definition site.
    /// Resolves bare names (locals, params, globals, builtins) and, best-effort,
    /// `base.member` attribute accesses (fields, methods, enum variants).
    pub fn symbol_at(&self, pos: Pos) -> Option<SymbolRef> {
        let nref = self.innermost_name_ref(pos)?;
        let ty = self
            .type_of_span(nref.span)
            .or_else(|| self.type_at(pos))
            .unwrap_or(Ty::Unknown);

        // Attribute access: resolve the member through the base's inferred type.
        if let Some(base_span) = nref.base_span {
            let (kind, def) = self.resolve_member(base_span, &nref.name);
            return Some(SymbolRef {
                name: nref.name,
                kind,
                ty,
                def,
            });
        }

        // A bare name: local first (scope-accurate, from the checker), then
        // global, then builtin.
        if let Some(def) = self.local_def_of(nref.span) {
            let kind = self.local_kind(def);
            let ty = self.local_ty(def).unwrap_or(ty);
            return Some(SymbolRef {
                name: nref.name,
                kind,
                ty,
                def: Some(def),
            });
        }
        if let Some(g) = self.global_defs.get(&nref.name) {
            return Some(SymbolRef {
                name: nref.name,
                kind: g.kind,
                ty,
                def: Some(g.span),
            });
        }
        Some(SymbolRef {
            name: nref.name,
            kind: SymbolKind::Builtin,
            ty,
            def: None,
        })
    }

    /// The definition site of the name under the cursor (go-to-def).
    pub fn definition_of(&self, pos: Pos) -> Option<Span> {
        self.symbol_at(pos).and_then(|s| s.def)
    }

    /// Every in-document use site of the symbol under the cursor, including its
    /// definition. For a local this is scope-accurate (two locals that share a
    /// name in different scopes are not conflated); for a global it is every
    /// bare reference by that name plus the declaration.
    pub fn references(&self, pos: Pos) -> Vec<Span> {
        let Some(nref) = self.innermost_name_ref(pos) else {
            return Vec::new();
        };
        // Local: group by the shared definition span recorded by the checker.
        if let Some(def) = self.local_def_of(nref.span) {
            let mut out: Vec<Span> = self
                .facts
                .resolutions
                .iter()
                .filter(|(_, d)| *d == def)
                .map(|(u, _)| *u)
                .collect();
            if !out.contains(&def) {
                out.push(def);
            }
            out.sort_by_key(|s| (s.start, s.end));
            out.dedup_by_key(|s| (s.start, s.end));
            return out;
        }
        // Global: every bare reference by this name, plus its declaration.
        let mut out: Vec<Span> = self
            .name_refs
            .iter()
            .filter(|r| r.base_span.is_none() && r.name == nref.name)
            .map(|r| r.span)
            .collect();
        if let Some(g) = self.global_defs.get(&nref.name) {
            if !out.contains(&g.span) {
                out.push(g.span);
            }
        }
        out.sort_by_key(|s| (s.start, s.end));
        out.dedup_by_key(|s| (s.start, s.end));
        out
    }

    // --- internals ---

    /// The smallest name reference whose span covers `pos`.
    fn innermost_name_ref(&self, pos: Pos) -> Option<NameRef> {
        self.name_refs
            .iter()
            .filter(|r| covers(r.span, pos))
            .min_by_key(|r| width(r.span))
            .cloned()
    }

    /// The definition span of the local binding a *use* at `use_span` resolved
    /// to, if it was a local (not a global).
    fn local_def_of(&self, use_span: Span) -> Option<Span> {
        self.facts
            .resolutions
            .iter()
            .find(|(u, _)| *u == use_span)
            .map(|(_, d)| *d)
    }

    fn local_kind(&self, def: Span) -> SymbolKind {
        if let Some((_, _, true)) = self.facts.local_defs.iter().find(|(d, _, _)| *d == def) {
            return SymbolKind::Param;
        }
        // A module-scope binding whose definition coincides with a top-level
        // declaration (an import) is reported with that declaration's kind.
        if let Some(g) = self.global_defs.values().find(|g| g.span == def) {
            return g.kind;
        }
        SymbolKind::Local
    }

    fn local_ty(&self, def: Span) -> Option<Ty> {
        self.facts
            .local_defs
            .iter()
            .find(|(d, _, _)| *d == def)
            .map(|(_, ty, _)| ty.clone())
    }

    /// Resolve `base.member`: find the base's inferred type, and if it names a
    /// user struct/enum/interface, look up the member's definition.
    fn resolve_member(&self, base_span: Span, member: &str) -> (SymbolKind, Option<Span>) {
        let Some(ty) = self.type_of_span(base_span) else {
            return (SymbolKind::Field, None);
        };
        let Some(type_name) = type_base_name(&ty) else {
            return (SymbolKind::Field, None);
        };
        let def = self
            .member_defs
            .get(&(type_name.clone(), member.to_string()))
            .copied();
        // Kind: a variant if the owner is an enum, else default to field; a
        // method if the member span is one we recorded as a method.
        let kind = match self.global_defs.get(&type_name).map(|g| g.kind) {
            Some(SymbolKind::Enum) => SymbolKind::EnumVariant,
            _ => SymbolKind::Field,
        };
        (kind, def)
    }
}

/// The base type name of `ty` for member resolution (`Point`, `Shape`, …), or
/// `None` for a type with no user-declared members.
fn type_base_name(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Named(n) => Some(n.clone()),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn make_diag(
    index: &LineIndex,
    severity: Severity,
    phase: &'static str,
    code: ErrorCode,
    message: String,
    span: Span,
) -> Diagnostic {
    let (line, col) = index.line_col(span.start);
    Diagnostic {
        severity,
        phase,
        code,
        message,
        span,
        line,
        col,
    }
}

/// A pure AST walk collecting the outline, global/member definitions, and every
/// bare-name reference. No checker state — so it works on an errored parse.
#[derive(Default)]
struct Walk {
    symbols: Vec<DocumentSymbol>,
    global_defs: HashMap<String, GlobalDef>,
    member_defs: HashMap<(String, String), Span>,
    name_refs: Vec<NameRef>,
}

impl Walk {
    fn module(&mut self, m: &Module) {
        for item in &m.items {
            self.item(item);
        }
    }

    fn item(&mut self, item: &Item) {
        match item {
            Item::Function(f) => {
                self.global_defs.insert(
                    f.name.clone(),
                    GlobalDef {
                        span: f.span,
                        kind: SymbolKind::Function,
                    },
                );
                let sym = self.function_symbol(f, SymbolKind::Function);
                self.symbols.push(sym);
            }
            Item::Struct(s) => {
                self.global_defs.insert(
                    s.name.clone(),
                    GlobalDef {
                        span: s.span,
                        kind: SymbolKind::Struct,
                    },
                );
                let mut children = Vec::new();
                for field in &s.fields {
                    self.member_defs
                        .insert((s.name.clone(), field.name.clone()), field.span);
                    children.push(DocumentSymbol {
                        name: field.name.clone(),
                        kind: SymbolKind::Field,
                        span: field.span,
                        name_span: field.span,
                        detail: Some(render_type(&field.ty)),
                        children: Vec::new(),
                    });
                    if let Some(d) = &field.default {
                        self.expr(d);
                    }
                }
                for m in &s.methods {
                    self.member_defs
                        .insert((s.name.clone(), m.name.clone()), m.span);
                    children.push(self.function_symbol(m, SymbolKind::Method));
                }
                self.symbols.push(DocumentSymbol {
                    name: s.name.clone(),
                    kind: SymbolKind::Struct,
                    span: s.span,
                    name_span: s.span,
                    detail: None,
                    children,
                });
            }
            Item::Enum(e) => {
                self.global_defs.insert(
                    e.name.clone(),
                    GlobalDef {
                        span: e.span,
                        kind: SymbolKind::Enum,
                    },
                );
                let mut children = Vec::new();
                for v in &e.variants {
                    self.member_defs
                        .insert((e.name.clone(), v.name.clone()), v.span);
                    children.push(DocumentSymbol {
                        name: v.name.clone(),
                        kind: SymbolKind::EnumVariant,
                        span: v.span,
                        name_span: v.span,
                        detail: None,
                        children: Vec::new(),
                    });
                }
                for m in &e.methods {
                    self.member_defs
                        .insert((e.name.clone(), m.name.clone()), m.span);
                    children.push(self.function_symbol(m, SymbolKind::Method));
                }
                self.symbols.push(DocumentSymbol {
                    name: e.name.clone(),
                    kind: SymbolKind::Enum,
                    span: e.span,
                    name_span: e.span,
                    detail: None,
                    children,
                });
            }
            Item::Interface(i) => {
                self.global_defs.insert(
                    i.name.clone(),
                    GlobalDef {
                        span: i.span,
                        kind: SymbolKind::Interface,
                    },
                );
                let mut children = Vec::new();
                for m in &i.methods {
                    self.member_defs
                        .insert((i.name.clone(), m.name.clone()), m.span);
                    children.push(DocumentSymbol {
                        name: m.name.clone(),
                        kind: SymbolKind::Method,
                        span: m.span,
                        name_span: m.span,
                        detail: None,
                        children: Vec::new(),
                    });
                }
                self.symbols.push(DocumentSymbol {
                    name: i.name.clone(),
                    kind: SymbolKind::Interface,
                    span: i.span,
                    name_span: i.span,
                    detail: None,
                    children,
                });
            }
            Item::Annotation(a) => {
                self.global_defs.insert(
                    a.name.clone(),
                    GlobalDef {
                        span: a.span,
                        kind: SymbolKind::Annotation,
                    },
                );
                self.symbols.push(DocumentSymbol {
                    name: a.name.clone(),
                    kind: SymbolKind::Annotation,
                    span: a.span,
                    name_span: a.span,
                    detail: None,
                    children: Vec::new(),
                });
            }
            Item::Decorator(d) => {
                self.global_defs.insert(
                    d.name.clone(),
                    GlobalDef {
                        span: d.span,
                        kind: SymbolKind::Decorator,
                    },
                );
                self.symbols.push(DocumentSymbol {
                    name: d.name.clone(),
                    kind: SymbolKind::Decorator,
                    span: d.span,
                    name_span: d.span,
                    detail: None,
                    children: Vec::new(),
                });
                self.block(&d.body);
            }
            Item::Import(im) => {
                let (name, span) = match im {
                    Import::Module {
                        path, alias, span, ..
                    } => (alias.clone().unwrap_or_else(|| path.clone()), *span),
                    Import::Python {
                        module,
                        alias,
                        span,
                    } => (alias.clone().unwrap_or_else(|| module.clone()), *span),
                };
                self.global_defs.insert(
                    name.clone(),
                    GlobalDef {
                        span,
                        kind: SymbolKind::Import,
                    },
                );
                self.symbols.push(DocumentSymbol {
                    name,
                    kind: SymbolKind::Import,
                    span,
                    name_span: span,
                    detail: None,
                    children: Vec::new(),
                });
            }
            Item::Stmt(st) => self.stmt(st),
        }
    }

    fn function_symbol(&mut self, f: &Function, kind: SymbolKind) -> DocumentSymbol {
        // Walk parameter defaults and the body for name references.
        for p in &f.params {
            if let Some(d) = &p.default {
                self.expr(d);
            }
        }
        self.block(&f.body);
        let detail = {
            let params: Vec<String> = f
                .params
                .iter()
                .map(|p| match &p.ty {
                    Some(t) => format!("{} {}", render_type(t), p.name),
                    None => p.name.clone(),
                })
                .collect();
            Some(format!("({})", params.join(", ")))
        };
        DocumentSymbol {
            name: f.name.clone(),
            kind,
            span: f.span,
            name_span: f.span,
            detail,
            children: Vec::new(),
        }
    }

    fn block(&mut self, b: &Block) {
        for st in &b.stmts {
            self.stmt(st);
        }
    }

    fn stmt(&mut self, st: &Stmt) {
        match st {
            Stmt::Let { value, .. } => self.expr(value),
            Stmt::Assign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::AugAssign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::Expr(e) => self.expr(e),
            Stmt::If {
                branches,
                else_block,
                ..
            } => {
                for (cond, body) in branches {
                    self.expr(cond);
                    self.block(body);
                }
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond);
                self.block(body);
            }
            Stmt::DoWhile { body, cond, .. } => {
                self.block(body);
                self.expr(cond);
            }
            Stmt::For {
                target, iter, body, ..
            } => {
                self.expr(target);
                self.expr(iter);
                self.block(body);
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.expr(g);
                    }
                    self.block(&arm.body);
                }
            }
            Stmt::Return { value, .. } | Stmt::Raise { value, .. } => {
                if let Some(e) = value {
                    self.expr(e);
                }
            }
            Stmt::Assert { cond, msg, .. } => {
                self.expr(cond);
                if let Some(m) = msg {
                    self.expr(m);
                }
            }
            Stmt::Try {
                body,
                excepts,
                finally,
                ..
            } => {
                self.block(body);
                for ex in excepts {
                    self.block(&ex.body);
                }
                if let Some(f) = finally {
                    self.block(f);
                }
            }
            Stmt::With { items, body, .. } => {
                for it in items {
                    self.expr(&it.expr);
                }
                self.block(body);
            }
            Stmt::Spawn { call, .. } => self.expr(call),
            Stmt::Scope { opts, body, .. } => {
                for a in opts {
                    self.expr(&a.value);
                }
                self.block(body);
            }
            Stmt::LetTuple { value, .. } | Stmt::AssignTuple { value, .. } => self.expr(value),
            Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Name(n, span) => self.name_refs.push(NameRef {
                name: n.clone(),
                span: *span,
                base_span: None,
            }),
            Expr::Attr { base, name, span } => {
                self.expr(base);
                // The member-name span is the tail of the attr span; use the
                // whole attr span (name-precise attr spans arrive with #225).
                self.name_refs.push(NameRef {
                    name: name.clone(),
                    span: *span,
                    base_span: Some(expr_span(base)),
                });
            }
            Expr::Tuple(xs, _) | Expr::List(xs, _) | Expr::Set(xs, _) => {
                for x in xs {
                    self.expr(x);
                }
            }
            Expr::Dict(pairs, _) => {
                for (k, v) in pairs {
                    self.expr(k);
                    self.expr(v);
                }
            }
            Expr::ListComp {
                elem, iter, cond, ..
            }
            | Expr::SetComp {
                elem, iter, cond, ..
            } => {
                self.expr(elem);
                self.expr(iter);
                if let Some(c) = cond {
                    self.expr(c);
                }
            }
            Expr::DictComp {
                key,
                value,
                iter,
                cond,
                ..
            } => {
                self.expr(key);
                self.expr(value);
                self.expr(iter);
                if let Some(c) = cond {
                    self.expr(c);
                }
            }
            Expr::Unary { expr, .. } => self.expr(expr),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::Call { callee, args, .. } => {
                self.expr(callee);
                for a in args {
                    self.expr(&a.value);
                }
            }
            Expr::Index { base, index, .. } => {
                self.expr(base);
                self.expr(index);
            }
            Expr::Slice {
                base, lo, hi, step, ..
            } => {
                self.expr(base);
                for o in [lo, hi, step].into_iter().flatten() {
                    self.expr(o);
                }
            }
            Expr::Lambda { body, .. } => self.expr(body),
            Expr::RegexMatch { subject, .. } | Expr::RegexSubst { subject, .. } => {
                self.expr(subject)
            }
            Expr::ChanRecv { chan, .. } => self.expr(chan),
            Expr::Yield { value, .. } => {
                if let Some(v) = value {
                    self.expr(v);
                }
            }
            Expr::IfElse {
                cond, then, els, ..
            } => {
                self.expr(cond);
                self.expr(then);
                self.expr(els);
            }
            // Literals carry no name references.
            Expr::Int(..)
            | Expr::Float(..)
            | Expr::Imaginary(..)
            | Expr::Str { .. }
            | Expr::Bool(..)
            | Expr::None(_) => {}
        }
    }
}

/// A readable rendering of a type annotation (for document-symbol detail).
fn render_type(t: &Type) -> String {
    match t {
        Type::Named { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner: Vec<String> = args.iter().map(render_type).collect();
                format!("{}[{}]", name, inner.join(", "))
            }
        }
        Type::Tuple { elems, .. } => {
            let inner: Vec<String> = elems.iter().map(render_type).collect();
            format!("({})", inner.join(", "))
        }
        Type::Func { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(render_type).collect();
            match ret {
                Some(r) => format!("fn({}) -> {}", ps.join(", "), render_type(r)),
                None => format!("fn({})", ps.join(", ")),
            }
        }
    }
}

/// The span of any expression (mirrors the private helper in `zapc_sema`).
fn expr_span(e: &Expr) -> Span {
    match e {
        Expr::Int(_, s)
        | Expr::Float(_, s)
        | Expr::Imaginary(_, s)
        | Expr::Bool(_, s)
        | Expr::None(s)
        | Expr::Name(_, s)
        | Expr::Tuple(_, s)
        | Expr::List(_, s)
        | Expr::Dict(_, s)
        | Expr::Set(_, s) => *s,
        Expr::Str { span, .. }
        | Expr::ListComp { span, .. }
        | Expr::DictComp { span, .. }
        | Expr::SetComp { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Call { span, .. }
        | Expr::Index { span, .. }
        | Expr::Slice { span, .. }
        | Expr::Attr { span, .. }
        | Expr::Lambda { span, .. }
        | Expr::RegexMatch { span, .. }
        | Expr::RegexSubst { span, .. }
        | Expr::ChanRecv { span, .. }
        | Expr::Yield { span, .. }
        | Expr::IfElse { span, .. } => *span,
    }
}
