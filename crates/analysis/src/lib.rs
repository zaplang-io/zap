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
    /// A readable one-line declaration signature, for symbols whose [`Ty`] does
    /// not capture their shape — functions and methods (`def f(int64 x) -> T`),
    /// and the header of a type (`struct Point`). `None` for locals/params,
    /// where `name: ty` already says everything, and for builtins.
    pub signature: Option<String>,
    /// The symbol's docstring (D-0041), if it has one (functions/methods).
    pub doc: Option<String>,
}

/// What kind of candidate a [`Completion`] is. Maps to the LSP
/// `CompletionItemKind` in the server layer (#230).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Keyword,
    Function,
    Method,
    Field,
    Struct,
    Enum,
    EnumVariant,
    Interface,
    Param,
    Local,
    Import,
}

/// A single completion candidate: what to insert, what kind of thing it is, and
/// an optional detail (a type or signature) for the client to show alongside.
#[derive(Clone, Debug)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
}

/// The keywords offered in scope (non-member) completion context. A fixed,
/// deliberately small set — the surface syntax keywords a cursor in statement
/// position might want. No fuzzy ranking; the server filters by prefix.
const KEYWORDS: &[&str] = &[
    "def",
    "struct",
    "enum",
    "interface",
    "annotation",
    "decorator",
    "import",
    "auto",
    "const",
    "return",
    "if",
    "elif",
    "else",
    "for",
    "while",
    "do",
    "match",
    "case",
    "try",
    "except",
    "finally",
    "raise",
    "assert",
    "with",
    "spawn",
    "scope",
    "break",
    "continue",
    "yield",
    "and",
    "or",
    "not",
    "in",
    "is",
    "true",
    "false",
    "None",
    "self",
];

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

/// Presentation facts about a definition, keyed by its definition span: a
/// readable signature and (for functions) a docstring. Built by the AST walk so
/// hover never needs the checker for a function's shape.
#[derive(Clone, Debug, Default)]
struct DefInfo {
    signature: Option<String>,
    doc: Option<String>,
}

/// A local binding (parameter, `let`, loop/tuple/comprehension/match target)
/// discovered by the AST walk, used to offer scope completions. `scope` is the
/// span of the enclosing function (or `None` at module level); `def.start`
/// orders a binding against the cursor.
#[derive(Clone, Debug)]
struct Binding {
    name: String,
    def: Span,
    kind: SymbolKind,
    scope: Option<Span>,
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
    /// Definition span → signature/docstring, for hover.
    def_info: HashMap<Span, DefInfo>,
    /// Local bindings (params, lets, loop vars, …), for scope completion.
    bindings: Vec<Binding>,
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
            def_info: walk.def_info,
            bindings: walk.bindings,
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
            let (signature, doc) = self.def_presentation(def);
            return Some(SymbolRef {
                name: nref.name,
                kind,
                ty,
                def,
                signature,
                doc,
            });
        }

        // A bare name: local first (scope-accurate, from the checker), then
        // global, then builtin.
        if let Some(def) = self.local_def_of(nref.span) {
            let kind = self.local_kind(def);
            let ty = self.local_ty(def).unwrap_or(ty);
            let (signature, doc) = self.def_presentation(Some(def));
            return Some(SymbolRef {
                name: nref.name,
                kind,
                ty,
                def: Some(def),
                signature,
                doc,
            });
        }
        if let Some(g) = self.global_defs.get(&nref.name) {
            let (signature, doc) = self.def_presentation(Some(g.span));
            return Some(SymbolRef {
                name: nref.name,
                kind: g.kind,
                ty,
                def: Some(g.span),
                signature,
                doc,
            });
        }
        Some(SymbolRef {
            name: nref.name,
            kind: SymbolKind::Builtin,
            ty,
            def: None,
            signature: None,
            doc: None,
        })
    }

    /// The signature and docstring recorded for a definition span, if any.
    fn def_presentation(&self, def: Option<Span>) -> (Option<String>, Option<String>) {
        match def.and_then(|d| self.def_info.get(&d)) {
            Some(info) => (info.signature.clone(), info.doc.clone()),
            None => (None, None),
        }
    }

    /// The definition site of the name under the cursor (go-to-def).
    pub fn definition_of(&self, pos: Pos) -> Option<Span> {
        self.symbol_at(pos).and_then(|s| s.def)
    }

    /// The span of the name reference under the cursor — the token a hover or
    /// highlight should cover. For `base.member` this is the whole attribute
    /// span (tightens to the member alone when name-precise spans land, #225).
    pub fn name_range_at(&self, pos: Pos) -> Option<Span> {
        self.innermost_name_ref(pos).map(|r| r.span)
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

    /// Completion candidates for the cursor at `pos`.
    ///
    /// Two contexts, chosen by looking at the characters just left of the
    /// cursor: if an identifier prefix is immediately preceded by `.`, it is
    /// **member** completion (the receiver's fields/methods/variants); otherwise
    /// it is **scope** completion (in-scope names plus keywords). The typed
    /// prefix filters the result; there is no fuzzy ranking.
    pub fn completions(&self, pos: Pos) -> Vec<Completion> {
        let pos = pos.min(self.source.len() as u32);
        let bytes = self.source.as_bytes();
        // Walk back over the in-progress identifier prefix.
        let mut start = pos as usize;
        while start > 0 && is_ident_byte(bytes[start - 1]) {
            start -= 1;
        }
        let prefix = &self.source[start..pos as usize];

        if start > 0 && bytes[start - 1] == b'.' {
            self.member_completions((start - 1) as u32, pos, prefix)
        } else {
            self.scope_completions(pos, prefix)
        }
    }

    /// Member completions after a `.`: the fields/methods/variants of the
    /// receiver's inferred type. The trailing `.` makes the document fail to
    /// parse, so this re-analyses a length-preserving *repaired* copy — the `.`
    /// and any partial member blanked to spaces, keeping every byte offset
    /// stable — reads the receiver type with [`type_at`](Self::type_at), and
    /// lists that type's members.
    fn member_completions(&self, dot: u32, pos: u32, prefix: &str) -> Vec<Completion> {
        let mut repaired = self.source.clone().into_bytes();
        for b in &mut repaired[dot as usize..pos as usize] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
        let repaired = match String::from_utf8(repaired) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let a = Analysis::analyze(&repaired);
        // The receiver ends just before the `.`.
        let Some(ty) = a.type_at(dot.saturating_sub(1)) else {
            return Vec::new();
        };
        let Some(type_name) = type_base_name(&ty) else {
            return Vec::new();
        };
        a.members_of(&type_name)
            .into_iter()
            .filter(|c| c.label.starts_with(prefix))
            .collect()
    }

    /// The fields/methods/variants of a user type, as completions (used by
    /// member completion). Reads the type's outline children; a method's detail
    /// is its full signature.
    fn members_of(&self, type_name: &str) -> Vec<Completion> {
        let Some(sym) = self.doc_symbols.iter().find(|s| {
            s.name == type_name
                && matches!(
                    s.kind,
                    SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Interface
                )
        }) else {
            return Vec::new();
        };
        sym.children
            .iter()
            .map(|c| Completion {
                label: c.name.clone(),
                kind: completion_kind(c.kind),
                detail: self
                    .def_info
                    .get(&c.span)
                    .and_then(|i| i.signature.clone())
                    .or_else(|| c.detail.clone()),
            })
            .collect()
    }

    /// Scope (non-member) completions: in-scope globals and locals plus
    /// keywords, filtered by the typed prefix. Visibility is conservative — a
    /// binding is offered when it is a global, or when its enclosing function
    /// covers the cursor and its definition precedes it (params always). This
    /// may over-offer across sibling inner scopes; it never ranks fuzzily.
    fn scope_completions(&self, pos: Pos, prefix: &str) -> Vec<Completion> {
        let mut out: Vec<Completion> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Locals and params of the enclosing scope first (nearest wins).
        for b in &self.bindings {
            let visible = match b.scope {
                Some(scope) => covers(scope, pos) && b.def.start <= pos,
                None => b.def.start <= pos,
            };
            if !visible || !b.name.starts_with(prefix) || !seen.insert(b.name.clone()) {
                continue;
            }
            out.push(Completion {
                label: b.name.clone(),
                kind: completion_kind(b.kind),
                detail: self.local_ty(b.def).map(|t| t.display()),
            });
        }

        // Top-level globals (functions, types, imports).
        for s in &self.doc_symbols {
            if !s.name.starts_with(prefix) || !seen.insert(s.name.clone()) {
                continue;
            }
            out.push(Completion {
                label: s.name.clone(),
                kind: completion_kind(s.kind),
                detail: self
                    .def_info
                    .get(&s.span)
                    .and_then(|i| i.signature.clone())
                    .or_else(|| s.detail.clone()),
            });
        }

        // Keywords.
        for kw in KEYWORDS {
            if kw.starts_with(prefix) && seen.insert((*kw).to_string()) {
                out.push(Completion {
                    label: (*kw).to_string(),
                    kind: CompletionKind::Keyword,
                    detail: None,
                });
            }
        }
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

/// Is `b` a byte that can appear in an identifier (`[A-Za-z0-9_]`)?
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Map an analysis [`SymbolKind`] to the completion kind presented to the
/// client. (`Annotation`/`Decorator`/`Builtin`/`TypeParam` fall back to
/// `Function`, the closest LSP bucket for a callable declaration.)
fn completion_kind(k: SymbolKind) -> CompletionKind {
    match k {
        SymbolKind::Function => CompletionKind::Function,
        SymbolKind::Method => CompletionKind::Method,
        SymbolKind::Param => CompletionKind::Param,
        SymbolKind::Local => CompletionKind::Local,
        SymbolKind::Struct => CompletionKind::Struct,
        SymbolKind::Enum => CompletionKind::Enum,
        SymbolKind::EnumVariant => CompletionKind::EnumVariant,
        SymbolKind::Field => CompletionKind::Field,
        SymbolKind::Interface => CompletionKind::Interface,
        SymbolKind::Import => CompletionKind::Import,
        SymbolKind::Annotation | SymbolKind::Decorator | SymbolKind::Builtin => {
            CompletionKind::Function
        }
    }
}

/// A readable one-line signature for a function/method declaration, matching
/// Zap's type-first surface syntax: `def name[generics](TYPE p, …) -> ret`.
///
/// Public so other frontend tooling (e.g. `zap doc`, I#232) renders function
/// signatures identically to hover; generic *bounds* are omitted here (as in
/// hover) and surfaced structurally by consumers that need them.
pub fn render_fn_signature(f: &Function) -> String {
    let generics = if f.generics.is_empty() {
        String::new()
    } else {
        let gs: Vec<String> = f.generics.iter().map(|g| g.name.clone()).collect();
        format!("[{}]", gs.join(", "))
    };
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| match &p.ty {
            Some(t) if p.variadic => format!("{}... {}", render_type(t), p.name),
            Some(t) => format!("{} {}", render_type(t), p.name),
            None => p.name.clone(),
        })
        .collect();
    let ret = match &f.ret {
        Some(r) => format!(" -> {}", render_type(r)),
        None => String::new(),
    };
    format!("def {}{}({}){}", f.name, generics, params.join(", "), ret)
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
    def_info: HashMap<Span, DefInfo>,
    bindings: Vec<Binding>,
    /// The enclosing function's span while walking a body (`None` at module
    /// level), so a binding records the scope it belongs to.
    cur_scope: Option<Span>,
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
                self.def_info.insert(
                    s.span,
                    DefInfo {
                        signature: Some(format!("struct {}", s.name)),
                        doc: None,
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
                self.def_info.insert(
                    e.span,
                    DefInfo {
                        signature: Some(format!("enum {}", e.name)),
                        doc: None,
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
                self.def_info.insert(
                    i.span,
                    DefInfo {
                        signature: Some(format!("interface {}", i.name)),
                        doc: None,
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
        // Signature + docstring for hover, keyed by the declaration span.
        self.def_info.insert(
            f.span,
            DefInfo {
                signature: Some(render_fn_signature(f)),
                doc: docstring(f).map(|d| d.to_string()),
            },
        );
        // Parameters are bindings in the function's scope.
        for p in &f.params {
            self.bindings.push(Binding {
                name: p.name.clone(),
                def: p.span,
                kind: SymbolKind::Param,
                scope: Some(f.span),
            });
        }
        // Walk parameter defaults and the body for name references and the
        // bindings introduced inside the body (which belong to this scope).
        for p in &f.params {
            if let Some(d) = &p.default {
                self.expr(d);
            }
        }
        let prev = self.cur_scope.replace(f.span);
        self.block(&f.body);
        self.cur_scope = prev;
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

    /// Record a binding in the current scope (for scope completion).
    fn bind(&mut self, name: String, def: Span, kind: SymbolKind) {
        self.bindings.push(Binding {
            name,
            def,
            kind,
            scope: self.cur_scope,
        });
    }

    /// Record every `Name` leaf of an assignment/loop target as a local binding
    /// (e.g. the `i` of `for i in …`, or `a`, `b` of `a, b = …`).
    fn bind_target_expr(&mut self, e: &Expr) {
        match e {
            Expr::Name(n, span) => self.bind(n.clone(), *span, SymbolKind::Local),
            Expr::Tuple(xs, _) | Expr::List(xs, _) => {
                for x in xs {
                    self.bind_target_expr(x);
                }
            }
            _ => {}
        }
    }

    /// Record the names bound by a tuple-destructuring target.
    fn bind_target(&mut self, t: &BindTarget) {
        match t {
            BindTarget::Elem(e) => self.bind_target_expr(e),
            BindTarget::Rest(inner, _) => self.bind_target(inner),
            BindTarget::Tuple(ts, _) => {
                for inner in ts {
                    self.bind_target(inner);
                }
            }
        }
    }

    /// Record the capture names bound by a match pattern.
    fn bind_pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Capture { name, span } => self.bind(name.clone(), *span, SymbolKind::Local),
            Pattern::Rest {
                name: Some(name),
                span,
            } => self.bind(name.clone(), *span, SymbolKind::Local),
            Pattern::Or(ps, _) | Pattern::Sequence(ps, _) | Pattern::Tuple(ps, _) => {
                for inner in ps {
                    self.bind_pattern(inner);
                }
            }
            Pattern::Struct {
                args: Some(args), ..
            } => {
                for a in args {
                    self.bind_pattern(&a.pattern);
                }
            }
            _ => {}
        }
    }

    fn stmt(&mut self, st: &Stmt) {
        match st {
            Stmt::Let {
                name, span, value, ..
            } => {
                self.bind(name.clone(), *span, SymbolKind::Local);
                self.expr(value);
            }
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
                self.bind_target_expr(target);
                self.expr(target);
                self.expr(iter);
                self.block(body);
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee);
                for arm in arms {
                    self.bind_pattern(&arm.pattern);
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
            Stmt::LetTuple { target, value, .. } => {
                self.bind_target(target);
                self.expr(value);
            }
            Stmt::AssignTuple { value, .. } => self.expr(value),
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
                elem,
                var,
                iter,
                cond,
                span,
            }
            | Expr::SetComp {
                elem,
                var,
                iter,
                cond,
                span,
            } => {
                self.bind(var.clone(), *span, SymbolKind::Local);
                self.expr(elem);
                self.expr(iter);
                if let Some(c) = cond {
                    self.expr(c);
                }
            }
            Expr::DictComp {
                key,
                value,
                var,
                iter,
                cond,
                span,
            } => {
                self.bind(var.clone(), *span, SymbolKind::Local);
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
///
/// Public so other frontend tooling (e.g. `zap doc`, I#232) renders type
/// annotations identically to hover/document-symbol detail.
pub fn render_type(t: &Type) -> String {
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
