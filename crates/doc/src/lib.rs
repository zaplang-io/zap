//! Zap API-documentation model extraction (I#157/C1, issue #232).
//!
//! This crate is the front half of `zap doc` (`design/0021-tooling.md` Area C,
//! `design/0023-doc-model.md`). It walks a parsed module and produces a stable,
//! serializable [`DocModel`]: the module and its documentable declarations —
//! functions, structs (with fields + methods), enums (with variants + methods),
//! interfaces, and top-level constants — each carrying a fully-qualified name, a
//! rendered signature, the attached docstring (D-0041), source location, and
//! visibility (D-0028). The JSON form of this model is the contract the #233
//! HTML/Markdown renderers consume; doctests (#234) are gated on the REPL (#159)
//! and out of scope here.
//!
//! **Reuse, not reimplementation.** Signatures render through
//! [`zapc_analysis::render_fn_signature`]/[`zapc_analysis::render_type`], so a
//! `zap doc` signature reads *identically* to an LSP hover. Source positions and
//! inferred constant types come from the same [`zapc_analysis::Analysis`] the LSP
//! runs. Extraction itself is a pure AST walk, so it degrades gracefully on a
//! file with type errors (like the document-symbol outline).
//!
//! **Visibility (D-0028).** Zap already enforces the rule "everything is public
//! except names starting with `_`, which are private" at compile time. `zap doc`
//! adopts exactly that rule: a leading-underscore name is [`Visibility::Private`]
//! and is omitted from the model unless [`DocOptions::include_private`] is set.
//! No new language notion is introduced.
//!
//! **Docstrings are module/def-level (SPEC §2, D-0041).** Module docstrings (a
//! leading bare triple-quoted statement) and function/method docstrings are
//! extracted. Zap's grammar has no docstring slot on a type today, and narrative
//! `#` comments need the A1 trivia work (#225), so the `doc` field on types,
//! fields, variants, and constants is reserved (always `None` for now) and will
//! populate without a schema change when either lands.
//!
//! **Serialization.** The toolchain ships no external dependencies beyond LLVM
//! (`driver/src/json.rs`: `zap mcp`/`zap lsp`/`zap test --json` all hand-roll
//! JSON), so this crate serializes with a small, self-contained, deterministic
//! writer rather than `serde`. [`DocModel::to_json_string`] emits pretty-printed,
//! insertion-ordered JSON.

use zapc_analysis::{render_fn_signature, render_type, Analysis};
use zapc_syntax::ast::*;
use zapc_syntax::span::Span;

mod json;
use json::Json;

// ----------------------------------------------------------------------------
// Options
// ----------------------------------------------------------------------------

/// How to build a [`DocModel`].
#[derive(Clone, Debug, Default)]
pub struct DocOptions {
    /// Include `_`-prefixed (private, D-0028) declarations. Default `false` — the
    /// model documents the public surface only, matching what another module can
    /// actually use.
    pub include_private: bool,
}

// ----------------------------------------------------------------------------
// Model
// ----------------------------------------------------------------------------

/// Whether a declaration is part of the public surface (D-0028).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

impl Visibility {
    /// The D-0028 rule: a leading-underscore name is private, everything else is
    /// public.
    pub fn of(name: &str) -> Visibility {
        if name.starts_with('_') {
            Visibility::Private
        } else {
            Visibility::Public
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Private => "private",
        }
    }
}

/// A docstring (D-0041), kept verbatim plus a light Google-style split (D-0054).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocString {
    /// The exact text between the triple quotes, unmodified.
    pub raw: String,
    /// The first paragraph, dedented and whitespace-collapsed — the one-line
    /// summary an index page shows.
    pub summary: String,
    /// The labelled sections (`Args:`/`Returns:`/`Raises:`/…) the Google-style
    /// convention defines, in source order. Empty when the docstring is a plain
    /// summary.
    pub sections: Vec<DocSection>,
}

/// One labelled block of a docstring (e.g. `Args:` and its body).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocSection {
    pub label: String,
    pub body: String,
}

/// A generic type parameter with its optional interface bound (D-0017).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generic {
    pub name: String,
    pub bound: Option<String>,
}

/// A function/method parameter (type-first, D-0003/D-0038).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocParam {
    pub name: String,
    /// The declared type, or `None` for an untyped `self` receiver.
    pub ty: Option<String>,
    /// The default value's source spelling, if any.
    pub default: Option<String>,
    pub variadic: bool,
}

/// A struct field (D-0015).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocField {
    pub name: String,
    pub ty: String,
    pub default: Option<String>,
    /// C-style bitfield width (I#173, D-0068), if any.
    pub bits: Option<u32>,
    pub visibility: Visibility,
    /// Reserved: fields have no docstring slot today (see the crate docs).
    pub doc: Option<DocString>,
}

/// A payload field of a data-carrying enum variant (D-0046).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocVariantField {
    pub name: String,
    pub ty: String,
}

/// An enum variant (D-0018 tier-1 constant or D-0046 data-carrying).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocVariant {
    pub name: String,
    /// An explicit tier-1 discriminant (`GREEN = 10`), as source text.
    pub value: Option<String>,
    pub fields: Vec<DocVariantField>,
    pub visibility: Visibility,
    /// Reserved: variants have no docstring slot today.
    pub doc: Option<DocString>,
}

/// The kind-specific body of a [`DocItem`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocItemBody {
    /// A top-level function or (nested) method.
    Function(DocFn),
    Method(DocFn),
    Struct(DocStruct),
    Enum(DocEnum),
    Interface(DocInterface),
    Const(DocConst),
}

impl DocItemBody {
    /// The JSON `kind` tag for this body. A `value struct` reports
    /// `"value_struct"` (and also carries `is_value: true`).
    fn kind_tag(&self) -> &'static str {
        match self {
            DocItemBody::Function(_) => "function",
            DocItemBody::Method(_) => "method",
            DocItemBody::Struct(s) if s.is_value => "value_struct",
            DocItemBody::Struct(_) => "struct",
            DocItemBody::Enum(_) => "enum",
            DocItemBody::Interface(_) => "interface",
            DocItemBody::Const(_) => "const",
        }
    }
}

/// A function or method declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocFn {
    /// The one-line signature, rendered identically to an LSP hover.
    pub signature: String,
    pub generics: Vec<Generic>,
    pub params: Vec<DocParam>,
    pub ret: Option<String>,
    pub decorators: Vec<String>,
}

/// A struct declaration (D-0015).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocStruct {
    pub signature: String,
    pub is_value: bool,
    pub generics: Vec<Generic>,
    pub decorators: Vec<String>,
    pub fields: Vec<DocField>,
    pub methods: Vec<DocItem>,
}

/// An enum declaration (D-0018/D-0046).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocEnum {
    pub signature: String,
    pub generics: Vec<Generic>,
    pub decorators: Vec<String>,
    pub variants: Vec<DocVariant>,
    pub methods: Vec<DocItem>,
}

/// An interface declaration (D-0016).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocInterface {
    pub signature: String,
    pub generics: Vec<Generic>,
    pub methods: Vec<DocItem>,
}

/// A top-level constant (`const …`, D-0003).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocConst {
    pub signature: String,
    /// The declared type, or the inferred type for an `auto`/untyped const when
    /// the program type-checks; `None` otherwise.
    pub ty: Option<String>,
}

/// One documentable declaration. `span`/`line`/`col` locate the declaration in
/// the source; nested methods/fields/variants hang off the body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocItem {
    pub name: String,
    /// Dotted path from the module, e.g. `shapes.Point.dist`.
    pub qualified_name: String,
    pub visibility: Visibility,
    /// Byte span of the whole declaration.
    pub span: Span,
    /// 1-based line of the declaration start.
    pub line: u32,
    /// 1-based column of the declaration start.
    pub col: u32,
    /// The attached docstring (functions/methods and the module only, today).
    pub doc: Option<DocString>,
    pub body: DocItemBody,
}

/// The module header of a [`DocModel`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocModuleInfo {
    /// The module's logical name (its file stem), if the caller supplied one.
    pub name: Option<String>,
    /// The module docstring — a leading bare triple-quoted statement (D-0041).
    pub doc: Option<DocString>,
}

/// The extracted documentation for one module: its header and its documentable
/// items, in declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocModel {
    pub module: DocModuleInfo,
    pub items: Vec<DocItem>,
}

impl DocModel {
    /// Extract the doc model from one source document. `module_name` is the
    /// module's logical name (typically the file stem); pass `None` for an
    /// anonymous source (e.g. stdin).
    pub fn from_source(source: &str, module_name: Option<&str>, opts: &DocOptions) -> DocModel {
        let (module, _perrs, _lerrs) = zapc_syntax::parser::parse_str(source);
        // The same analysis handle the LSP uses: for source positions and for
        // inferring an `auto` const's type. Extraction never *requires* a clean
        // type-check — it falls back to the AST.
        let analysis = Analysis::analyze(source);
        let mut x = Extractor {
            analysis: &analysis,
            opts,
            module_name,
        };
        x.model(&module, module_name)
    }

    /// Serialize to pretty-printed, deterministic JSON — the contract #233
    /// consumes (`design/0023-doc-model.md`).
    pub fn to_json_string(&self) -> String {
        self.to_json().to_pretty(0)
    }

    fn to_json(&self) -> Json {
        let mut module = Json::object();
        module.push("name", opt_str(self.module.name.as_deref()));
        module.push("doc", doc_json(&self.module.doc));
        Json::object_from(vec![
            ("module", module),
            (
                "items",
                Json::Arr(self.items.iter().map(DocItem::to_json).collect()),
            ),
        ])
    }
}

// ----------------------------------------------------------------------------
// Extraction
// ----------------------------------------------------------------------------

struct Extractor<'a> {
    analysis: &'a Analysis,
    opts: &'a DocOptions,
    module_name: Option<&'a str>,
}

impl Extractor<'_> {
    fn model(&mut self, m: &Module, module_name: Option<&str>) -> DocModel {
        let module_doc = module_docstring(m).map(parse_docstring);

        let mut items = Vec::new();
        for (i, item) in m.items.iter().enumerate() {
            // The module docstring statement is not itself a documentable item.
            if i == 0 && module_doc.is_some() {
                if let Item::Stmt(Stmt::Expr(Expr::Str { .. })) = item {
                    continue;
                }
            }
            if let Some(di) = self.item(item) {
                items.push(di);
            }
        }

        DocModel {
            module: DocModuleInfo {
                name: module_name.map(str::to_string),
                doc: module_doc,
            },
            items,
        }
    }

    /// The dotted path for a name under an optional owner type.
    fn qualify(&self, owner: Option<&str>, name: &str) -> String {
        match (self.module_name, owner) {
            (Some(m), Some(o)) => format!("{m}.{o}.{name}"),
            (Some(m), None) => format!("{m}.{name}"),
            (None, Some(o)) => format!("{o}.{name}"),
            (None, None) => name.to_string(),
        }
    }

    /// Is this name visible under the current options (D-0028)?
    fn visible(&self, name: &str) -> bool {
        self.opts.include_private || Visibility::of(name) == Visibility::Public
    }

    fn locate(&self, span: Span) -> (u32, u32) {
        self.analysis.line_col(span.start)
    }

    fn item(&mut self, item: &Item) -> Option<DocItem> {
        match item {
            Item::Function(f) if self.visible(&f.name) => Some(self.function_item(f, None, false)),
            Item::Struct(s) if self.visible(&s.name) => Some(self.struct_item(s)),
            Item::Enum(e) if self.visible(&e.name) => Some(self.enum_item(e)),
            Item::Interface(i) if self.visible(&i.name) => Some(self.interface_item(i)),
            Item::Stmt(Stmt::Let {
                is_const: true,
                ty,
                name,
                value,
                span,
            }) if self.visible(name) => Some(self.const_item(name, ty.as_ref(), value, *span)),
            _ => None,
        }
    }

    /// Build a function or (when `is_method`) method item, optionally owned by a
    /// type for qualification.
    fn function_item(&self, f: &Function, owner: Option<&str>, is_method: bool) -> DocItem {
        let (line, col) = self.locate(f.span);
        let doc_fn = DocFn {
            signature: render_fn_signature(f),
            generics: generics(&f.generics),
            params: f.params.iter().map(doc_param).collect(),
            ret: f.ret.as_ref().map(render_type),
            decorators: f.decorators.iter().map(render_decorator).collect(),
        };
        DocItem {
            name: f.name.clone(),
            qualified_name: self.qualify(owner, &f.name),
            visibility: Visibility::of(&f.name),
            span: f.span,
            line,
            col,
            doc: docstring(f).map(parse_docstring),
            body: if is_method {
                DocItemBody::Method(doc_fn)
            } else {
                DocItemBody::Function(doc_fn)
            },
        }
    }

    fn struct_item(&self, s: &StructDef) -> DocItem {
        let (line, col) = self.locate(s.span);
        let keyword = if s.is_value { "value struct" } else { "struct" };
        let fields = s
            .fields
            .iter()
            .filter(|f| self.visible(&f.name))
            .map(|f| DocField {
                name: f.name.clone(),
                ty: render_type(&f.ty),
                default: f.default.as_ref().map(render_expr),
                bits: f.bits,
                visibility: Visibility::of(&f.name),
                doc: None,
            })
            .collect();
        let methods = s
            .methods
            .iter()
            .filter(|m| self.visible(&m.name))
            .map(|m| self.function_item(m, Some(&s.name), true))
            .collect();
        DocItem {
            name: s.name.clone(),
            qualified_name: self.qualify(None, &s.name),
            visibility: Visibility::of(&s.name),
            span: s.span,
            line,
            col,
            doc: None,
            body: DocItemBody::Struct(DocStruct {
                signature: type_header(keyword, &s.name, &s.generics),
                is_value: s.is_value,
                generics: generics(&s.generics),
                decorators: s.decorators.iter().map(render_decorator).collect(),
                fields,
                methods,
            }),
        }
    }

    fn enum_item(&self, e: &EnumDef) -> DocItem {
        let (line, col) = self.locate(e.span);
        let variants = e
            .variants
            .iter()
            .filter(|v| self.visible(&v.name))
            .map(|v| DocVariant {
                name: v.name.clone(),
                value: v.value.as_ref().map(render_expr),
                fields: v
                    .fields
                    .iter()
                    .map(|vf| DocVariantField {
                        name: vf.name.clone(),
                        ty: render_type(&vf.ty),
                    })
                    .collect(),
                visibility: Visibility::of(&v.name),
                doc: None,
            })
            .collect();
        let methods = e
            .methods
            .iter()
            .filter(|m| self.visible(&m.name))
            .map(|m| self.function_item(m, Some(&e.name), true))
            .collect();
        DocItem {
            name: e.name.clone(),
            qualified_name: self.qualify(None, &e.name),
            visibility: Visibility::of(&e.name),
            span: e.span,
            line,
            col,
            doc: None,
            body: DocItemBody::Enum(DocEnum {
                signature: type_header("enum", &e.name, &e.generics),
                generics: generics(&e.generics),
                decorators: e.decorators.iter().map(render_decorator).collect(),
                variants,
                methods,
            }),
        }
    }

    fn interface_item(&self, i: &InterfaceDef) -> DocItem {
        let (line, col) = self.locate(i.span);
        let methods = i
            .methods
            .iter()
            .filter(|m| self.visible(&m.name))
            .map(|m| self.method_sig_item(i, m))
            .collect();
        DocItem {
            name: i.name.clone(),
            qualified_name: self.qualify(None, &i.name),
            visibility: Visibility::of(&i.name),
            span: i.span,
            line,
            col,
            doc: None,
            body: DocItemBody::Interface(DocInterface {
                signature: type_header("interface", &i.name, &i.generics),
                generics: generics(&i.generics),
                methods,
            }),
        }
    }

    /// An interface method signature (no body, so no docstring).
    fn method_sig_item(&self, owner: &InterfaceDef, m: &MethodSig) -> DocItem {
        let (line, col) = self.locate(m.span);
        let params: Vec<DocParam> = m.params.iter().map(doc_param).collect();
        let ret = m.ret.as_ref().map(render_type);
        DocItem {
            name: m.name.clone(),
            qualified_name: self.qualify(Some(&owner.name), &m.name),
            visibility: Visibility::of(&m.name),
            span: m.span,
            line,
            col,
            doc: None,
            body: DocItemBody::Method(DocFn {
                signature: method_sig_signature(m, &params, ret.as_deref()),
                generics: Vec::new(),
                params,
                ret,
                decorators: Vec::new(),
            }),
        }
    }

    fn const_item(&self, name: &str, ty: Option<&Type>, value: &Expr, span: Span) -> DocItem {
        let (line, col) = self.locate(span);
        // Declared type wins; otherwise ask the shared analysis for the inferred
        // type of the initializer, as the LSP would.
        let declared = ty.map(render_type);
        let ty_str = declared.clone().or_else(|| {
            let t = self.analysis.type_at(expr_start(value))?;
            Some(t.display())
        });
        let shown = ty_str.clone().unwrap_or_else(|| "auto".to_string());
        DocItem {
            name: name.to_string(),
            qualified_name: self.qualify(None, name),
            visibility: Visibility::of(name),
            span,
            line,
            col,
            doc: None,
            body: DocItemBody::Const(DocConst {
                signature: format!("const {shown} {name}"),
                ty: ty_str,
            }),
        }
    }
}

// ----------------------------------------------------------------------------
// Rendering helpers
// ----------------------------------------------------------------------------

fn generics(ps: &[TypeParam]) -> Vec<Generic> {
    ps.iter()
        .map(|g| Generic {
            name: g.name.clone(),
            bound: g.bound.as_ref().map(render_type),
        })
        .collect()
}

fn doc_param(p: &Param) -> DocParam {
    DocParam {
        name: p.name.clone(),
        ty: p.ty.as_ref().map(render_type),
        default: p.default.as_ref().map(render_expr),
        variadic: p.variadic,
    }
}

/// `keyword Name[G1, G2]` — the header line for a struct/enum/interface.
fn type_header(keyword: &str, name: &str, gs: &[TypeParam]) -> String {
    if gs.is_empty() {
        format!("{keyword} {name}")
    } else {
        let names: Vec<String> = gs.iter().map(|g| g.name.clone()).collect();
        format!("{keyword} {name}[{}]", names.join(", "))
    }
}

/// The one-line signature of an interface method, matching the function form.
fn method_sig_signature(m: &MethodSig, params: &[DocParam], ret: Option<&str>) -> String {
    let ps: Vec<String> = params
        .iter()
        .map(|p| match &p.ty {
            Some(t) if p.variadic => format!("{t}... {}", p.name),
            Some(t) => format!("{t} {}", p.name),
            None => p.name.clone(),
        })
        .collect();
    let ret = ret.map(|r| format!(" -> {r}")).unwrap_or_default();
    format!("def {}({}){ret}", m.name, ps.join(", "))
}

/// `@name` or `@name(arg, kw=val)` — a decorator application as source text.
fn render_decorator(d: &Decorator) -> String {
    if d.args.is_empty() {
        format!("@{}", d.name)
    } else {
        let args: Vec<String> = d
            .args
            .iter()
            .map(|a| match &a.name {
                Some(n) => format!("{n}={}", render_expr(&a.value)),
                None => render_expr(&a.value),
            })
            .collect();
        format!("@{}({})", d.name, args.join(", "))
    }
}

/// The module docstring: a leading bare non-f triple-quoted statement (D-0041).
fn module_docstring(m: &Module) -> Option<&str> {
    match m.items.first() {
        Some(Item::Stmt(Stmt::Expr(Expr::Str {
            value,
            fstring: false,
            ..
        }))) => Some(value.as_str()),
        _ => None,
    }
}

/// The start byte offset of an expression (for `type_at` on a const value).
fn expr_start(e: &Expr) -> u32 {
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
        | Expr::Set(_, s) => s.start,
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
        | Expr::IfElse { span, .. } => span.start,
    }
}

// ----------------------------------------------------------------------------
// Docstring parsing (D-0054 Google style)
// ----------------------------------------------------------------------------

/// Parse a raw docstring into a verbatim copy plus a dedented summary and the
/// Google-style labelled sections (`Args:`/`Returns:`/`Raises:`/…).
fn parse_docstring(raw: &str) -> DocString {
    let cleaned = cleandoc(raw);
    DocString {
        summary: summary_of(&cleaned),
        sections: parse_sections(&cleaned),
        raw: raw.to_string(),
    }
}

/// `inspect.cleandoc`-style normalization: drop a leading blank line, strip the
/// common leading indentation of the continuation lines, and trim surrounding
/// blank lines.
fn cleandoc(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    // Common indent across all lines after the first (the first line follows the
    // opening quotes, so it carries no indentation).
    let mut min_indent: Option<usize> = None;
    for line in &lines[1..] {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        min_indent = Some(min_indent.map_or(indent, |m| m.min(indent)));
    }
    let strip = min_indent.unwrap_or(0);
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    out.push(lines[0].trim().to_string());
    for line in &lines[1..] {
        if line.len() >= strip {
            out.push(line[strip..].trim_end().to_string());
        } else {
            out.push(line.trim_end().to_string());
        }
    }
    // Trim leading/trailing blank lines.
    while out.first().is_some_and(|l| l.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// The first paragraph of a cleaned docstring, collapsed to one line.
fn summary_of(cleaned: &str) -> String {
    let mut para: Vec<&str> = Vec::new();
    for line in cleaned.lines() {
        if line.trim().is_empty() {
            break;
        }
        para.push(line.trim());
    }
    para.join(" ")
}

/// Split a cleaned docstring into its labelled sections. A section header is a
/// line of the form `Label:` (a short run of letters/spaces, nothing after the
/// colon); its body is every following line up to the next header, dedented.
fn parse_sections(cleaned: &str) -> Vec<DocSection> {
    let mut sections: Vec<DocSection> = Vec::new();
    let mut cur: Option<(String, Vec<String>)> = None;
    for line in cleaned.lines() {
        if let Some(label) = section_label(line) {
            if let Some((l, body)) = cur.take() {
                sections.push(finish_section(l, body));
            }
            cur = Some((label, Vec::new()));
        } else if let Some((_, body)) = cur.as_mut() {
            body.push(line.to_string());
        }
    }
    if let Some((l, body)) = cur.take() {
        sections.push(finish_section(l, body));
    }
    sections
}

/// If `line` is a section header (`Label:` with nothing else), its label.
fn section_label(line: &str) -> Option<String> {
    let t = line.trim();
    let head = t.strip_suffix(':')?;
    if head.is_empty()
        || !head.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
        || head.contains("  ")
    {
        return None;
    }
    Some(head.to_string())
}

fn finish_section(label: String, body: Vec<String>) -> DocSection {
    // Dedent the body by its common indent and trim surrounding blanks.
    let min_indent = body
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut lines: Vec<String> = body
        .iter()
        .map(|l| {
            if l.len() >= min_indent {
                l[min_indent..].trim_end().to_string()
            } else {
                l.trim_end().to_string()
            }
        })
        .collect();
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    DocSection {
        label,
        body: lines.join("\n"),
    }
}

// ----------------------------------------------------------------------------
// JSON mapping
// ----------------------------------------------------------------------------

fn opt_str(s: Option<&str>) -> Json {
    match s {
        Some(s) => Json::str(s),
        None => Json::Null,
    }
}

fn doc_json(doc: &Option<DocString>) -> Json {
    match doc {
        None => Json::Null,
        Some(d) => Json::object_from(vec![
            ("raw", Json::str(&d.raw)),
            ("summary", Json::str(&d.summary)),
            (
                "sections",
                Json::Arr(
                    d.sections
                        .iter()
                        .map(|s| {
                            Json::object_from(vec![
                                ("label", Json::str(&s.label)),
                                ("body", Json::str(&s.body)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
    }
}

fn generics_json(gs: &[Generic]) -> Json {
    Json::Arr(
        gs.iter()
            .map(|g| {
                Json::object_from(vec![
                    ("name", Json::str(&g.name)),
                    ("bound", opt_str(g.bound.as_deref())),
                ])
            })
            .collect(),
    )
}

fn params_json(ps: &[DocParam]) -> Json {
    Json::Arr(
        ps.iter()
            .map(|p| {
                Json::object_from(vec![
                    ("name", Json::str(&p.name)),
                    ("type", opt_str(p.ty.as_deref())),
                    ("default", opt_str(p.default.as_deref())),
                    ("variadic", Json::Bool(p.variadic)),
                ])
            })
            .collect(),
    )
}

fn decorators_json(ds: &[String]) -> Json {
    Json::Arr(ds.iter().map(|d| Json::str(d)).collect())
}

impl DocItem {
    fn to_json(&self) -> Json {
        let mut o = Json::object();
        o.push("kind", Json::str(self.body.kind_tag()));
        o.push("name", Json::str(&self.name));
        o.push("qualified_name", Json::str(&self.qualified_name));
        o.push("visibility", Json::str(self.visibility.as_str()));
        o.push("line", Json::num(self.line));
        o.push("col", Json::num(self.col));
        o.push(
            "span",
            Json::object_from(vec![
                ("start", Json::num(self.span.start)),
                ("end", Json::num(self.span.end)),
            ]),
        );
        o.push("doc", doc_json(&self.doc));
        match &self.body {
            DocItemBody::Function(f) | DocItemBody::Method(f) => {
                o.push("signature", Json::str(&f.signature));
                o.push("generics", generics_json(&f.generics));
                o.push("params", params_json(&f.params));
                o.push("ret", opt_str(f.ret.as_deref()));
                o.push("decorators", decorators_json(&f.decorators));
            }
            DocItemBody::Struct(s) => {
                o.push("signature", Json::str(&s.signature));
                o.push("is_value", Json::Bool(s.is_value));
                o.push("generics", generics_json(&s.generics));
                o.push("decorators", decorators_json(&s.decorators));
                o.push("fields", fields_json(&s.fields));
                o.push("methods", methods_json(&s.methods));
            }
            DocItemBody::Enum(e) => {
                o.push("signature", Json::str(&e.signature));
                o.push("generics", generics_json(&e.generics));
                o.push("decorators", decorators_json(&e.decorators));
                o.push("variants", variants_json(&e.variants));
                o.push("methods", methods_json(&e.methods));
            }
            DocItemBody::Interface(i) => {
                o.push("signature", Json::str(&i.signature));
                o.push("generics", generics_json(&i.generics));
                o.push("methods", methods_json(&i.methods));
            }
            DocItemBody::Const(c) => {
                o.push("signature", Json::str(&c.signature));
                o.push("type", opt_str(c.ty.as_deref()));
            }
        }
        o
    }
}

fn methods_json(ms: &[DocItem]) -> Json {
    Json::Arr(ms.iter().map(DocItem::to_json).collect())
}

fn fields_json(fs: &[DocField]) -> Json {
    Json::Arr(
        fs.iter()
            .map(|f| {
                Json::object_from(vec![
                    ("name", Json::str(&f.name)),
                    ("type", Json::str(&f.ty)),
                    ("default", opt_str(f.default.as_deref())),
                    (
                        "bits",
                        match f.bits {
                            Some(b) => Json::num(b),
                            None => Json::Null,
                        },
                    ),
                    ("visibility", Json::str(f.visibility.as_str())),
                    ("doc", doc_json(&f.doc)),
                ])
            })
            .collect(),
    )
}

fn variants_json(vs: &[DocVariant]) -> Json {
    Json::Arr(
        vs.iter()
            .map(|v| {
                Json::object_from(vec![
                    ("name", Json::str(&v.name)),
                    ("value", opt_str(v.value.as_deref())),
                    (
                        "fields",
                        Json::Arr(
                            v.fields
                                .iter()
                                .map(|vf| {
                                    Json::object_from(vec![
                                        ("name", Json::str(&vf.name)),
                                        ("type", Json::str(&vf.ty)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    ("visibility", Json::str(v.visibility.as_str())),
                    ("doc", doc_json(&v.doc)),
                ])
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_rule_is_leading_underscore() {
        assert_eq!(Visibility::of("area"), Visibility::Public);
        assert_eq!(Visibility::of("_helper"), Visibility::Private);
    }

    #[test]
    fn cleandoc_dedents_and_trims() {
        let raw = "Summary line.\n\n    Args:\n        r: radius\n    ";
        let c = cleandoc(raw);
        assert!(c.starts_with("Summary line."));
        assert!(c.contains("Args:"));
        assert!(c.contains("    r: radius")); // relative indent preserved
    }

    #[test]
    fn sections_split_google_style() {
        let raw = "Area of a circle.\n\n    Args:\n        r: radius\n\n    Returns:\n        the area\n    ";
        let d = parse_docstring(raw);
        assert_eq!(d.summary, "Area of a circle.");
        let labels: Vec<&str> = d.sections.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["Args", "Returns"]);
        assert_eq!(d.sections[0].body, "r: radius");
        assert_eq!(d.sections[1].body, "the area");
    }
}
