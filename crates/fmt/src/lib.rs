//! The canonical Zap source formatter — `zap fmt` (I#157/A2, #226; D-0037).
//!
//! One true, Python-faithful style (gofmt in spirit): 4-space indent, a target
//! line width of [`MAX_WIDTH`], double-quote-preferred strings, blank-line
//! collapsing, and width-triggered wrapping of calls/collections/signatures.
//! It reuses the parser and the comment trivia from `zapc_syntax` (#225); it
//! never introduces a second parser.
//!
//! Two hard guarantees, enforced by the test suite:
//! - **Idempotence:** `format(format(x)) == format(x)`. Every style rule is a
//!   fixed point, and every wrapping decision is a pure function of the parsed
//!   AST (not of the incoming whitespace), so a second pass changes nothing.
//! - **Comment preservation:** every `#` comment survives, re-attached to the
//!   node it sat on (leading / trailing / dangling).
//!
//! The style is specified in `docs/FORMAT.md` and recorded in D-0037.

mod render;
mod strings;

use render::*;
use strings::{format_string, normalize_comment};
use zapc_syntax::ast::*;
use zapc_syntax::parser::parse_str_with_trivia;
use zapc_syntax::span::Span;
use zapc_syntax::trivia::{blank_lines_between, newlines_between, Comment, CommentPlacement};

/// The target maximum line width (D-0037). Lines that cannot be wrapped within
/// it (e.g. a single long string) are left over-length.
pub const MAX_WIDTH: usize = 88;

const INDENT: &str = "    ";

/// Why source could not be formatted: it did not lex/parse cleanly. The
/// formatter never rewrites invalid source (that would risk changing meaning).
#[derive(Clone, Debug)]
pub struct FmtError {
    pub messages: Vec<String>,
}

impl std::fmt::Display for FmtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot format source with errors:\n  {}",
            self.messages.join("\n  ")
        )
    }
}

impl std::error::Error for FmtError {}

/// Format Zap source into its canonical form. Errors if the source does not
/// parse cleanly (the caller should leave such files untouched).
pub fn format_source(src: &str) -> Result<String, FmtError> {
    let (module, perrs, lerrs, comments) = parse_str_with_trivia(src);
    if !lerrs.is_empty() || !perrs.is_empty() {
        let mut messages = Vec::new();
        for e in &lerrs {
            messages.push(format!("lex error at byte {}: {}", e.span.start, e.message));
        }
        for e in &perrs {
            messages.push(format!(
                "parse error at byte {}: {}",
                e.span.start, e.message
            ));
        }
        return Err(FmtError { messages });
    }
    let mut p = Printer::new(src, &comments);
    p.emit_module(&module);
    Ok(p.finish())
}

/// `true` if `src` is already in canonical form.
pub fn is_formatted(src: &str) -> bool {
    matches!(format_source(src), Ok(out) if out == src)
}

struct Printer<'a> {
    src: &'a str,
    comments: &'a [Comment],
    ci: usize,
    lines: Vec<String>,
    indent: usize,
    /// Byte offset of the end of the last emitted source construct — the anchor
    /// for computing how many author blank lines to preserve.
    last_end: u32,
    /// Suppress the leading blank line(s) of the first construct in a block.
    block_fresh: bool,
}

/// A struct/enum body member, kept in source order so comments stay attached.
enum Member<'a> {
    Field(&'a Field),
    Variant(&'a EnumVariant),
    Method(&'a Function),
}

impl<'a> Member<'a> {
    fn start(&self) -> u32 {
        match self {
            Member::Field(f) => f.decorators.first().map_or(f.span.start, |d| d.span.start),
            Member::Variant(v) => v.span.start,
            Member::Method(m) => m.decorators.first().map_or(m.span.start, |d| d.span.start),
        }
    }
}

impl<'a> Printer<'a> {
    fn new(src: &'a str, comments: &'a [Comment]) -> Self {
        Printer {
            src,
            comments,
            ci: 0,
            lines: Vec::new(),
            indent: 0,
            last_end: 0,
            block_fresh: true,
        }
    }

    fn finish(mut self) -> String {
        while matches!(self.lines.last(), Some(l) if l.is_empty()) {
            self.lines.pop();
        }
        if self.lines.is_empty() {
            return String::new();
        }
        let mut out = self.lines.join("\n");
        out.push('\n');
        out
    }

    // --- line + blank-line + comment machinery ---

    fn push_line(&mut self, text: &str) {
        if text.is_empty() {
            self.lines.push(String::new());
        } else {
            self.lines
                .push(format!("{}{}", INDENT.repeat(self.indent), text));
        }
    }

    fn fits(&self, line: &str) -> bool {
        self.indent * INDENT.len() + line.chars().count() <= MAX_WIDTH
    }

    fn gap(&mut self, start: u32, max: usize) {
        // Some nodes' spans end past the trailing newline; trim back to the last
        // non-whitespace byte so the blank-line count reflects the real gap.
        let bytes = self.src.as_bytes();
        let mut a = self.last_end as usize;
        while a > 0 && matches!(bytes.get(a - 1), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            a -= 1;
        }
        let n = blank_lines_between(self.src, a, start as usize).min(max);
        for _ in 0..n {
            self.lines.push(String::new());
        }
    }

    fn attach_trailing(&mut self, text: &str) {
        match self.lines.last_mut() {
            Some(l) if !l.is_empty() => {
                l.push_str("  ");
                l.push_str(text);
            }
            _ => self.push_line(text),
        }
    }

    /// Emit every pending comment whose span starts before `before`, in source
    /// order, with blank-line separation capped at `max`.
    fn flush_comments(&mut self, before: u32, max: usize) {
        while self.ci < self.comments.len() && self.comments[self.ci].span.start < before {
            let c = self.comments[self.ci].clone();
            self.ci += 1;
            match c.placement {
                CommentPlacement::OwnLine => {
                    if self.block_fresh {
                        self.block_fresh = false;
                    } else {
                        self.gap(c.span.start, max);
                    }
                    let line = normalize_comment(&c.text);
                    self.push_line(&line);
                }
                CommentPlacement::EndOfLine => {
                    let t = normalize_comment(&c.text);
                    self.attach_trailing(&t);
                }
            }
            self.last_end = c.span.end;
        }
    }

    /// Flush comments trailing inside a block, up to `before` (the dedent). Only
    /// comments indented *into* the block (column ≥ the body indent) are dangling
    /// here; a comment dedented to an outer level belongs to the following
    /// construct and is left for its leading flush, so block spans that run to the
    /// next token don't steal it.
    fn flush_dangling(&mut self, before: u32) {
        let min_col = self.indent * INDENT.len();
        while self.ci < self.comments.len() && self.comments[self.ci].span.start < before {
            if self.source_col(self.comments[self.ci].span.start) < min_col {
                break;
            }
            let c = self.comments[self.ci].clone();
            self.ci += 1;
            match c.placement {
                CommentPlacement::OwnLine => {
                    if self.block_fresh {
                        self.block_fresh = false;
                    } else {
                        self.gap(c.span.start, 1);
                    }
                    let line = normalize_comment(&c.text);
                    self.push_line(&line);
                }
                CommentPlacement::EndOfLine => {
                    let t = normalize_comment(&c.text);
                    self.attach_trailing(&t);
                }
            }
            self.last_end = c.span.end;
        }
    }

    /// The source column (in spaces) of byte offset `pos` — distance back to the
    /// start of its line. Indentation is spaces only (D-0041), so this is the
    /// indent level times four for a line-leading position.
    fn source_col(&self, pos: u32) -> usize {
        let p = pos as usize;
        let bytes = self.src.as_bytes();
        let mut i = p;
        while i > 0 && bytes[i - 1] != b'\n' {
            i -= 1;
        }
        p - i
    }

    /// Prepare to emit a construct beginning at byte `start`: flush any leading
    /// comments, then emit the blank-line gap before it (capped at `max`).
    fn advance_to(&mut self, start: u32, max: usize) {
        self.flush_comments(start, max);
        if self.block_fresh {
            self.block_fresh = false;
        } else {
            self.gap(start, max);
        }
    }

    /// Take a trailing (end-of-line) comment that sits on the same source line as
    /// a construct ending at `anchor_end`, and glue it onto the last line.
    fn take_trailing(&mut self, anchor_end: u32) {
        self.take_trailing_before(anchor_end, u32::MAX);
    }

    /// Like [`take_trailing`], but only if the comment starts before `before`.
    /// Used when wrapping a bracketed expression so a comment that actually
    /// trails the *whole* construct (after the close bracket, on the same source
    /// line) is not grabbed by the first element — it stops at the next element's
    /// start, or the close bracket.
    fn take_trailing_before(&mut self, anchor_end: u32, before: u32) {
        if self.ci < self.comments.len() {
            let c = &self.comments[self.ci];
            if c.placement == CommentPlacement::EndOfLine
                && c.span.start < before
                && newlines_between(self.src, anchor_end as usize, c.span.start as usize) == 0
            {
                let t = normalize_comment(&c.text);
                let end = c.span.end;
                self.ci += 1;
                self.attach_trailing(&t);
                self.last_end = end;
            }
        }
    }

    /// Take a trailing comment on a block-header line (`if cond:  # note`): the
    /// only code before the body is the header, so any end-of-line comment before
    /// `body_start` trails the header.
    fn take_header_trailing(&mut self, body_start: u32) {
        if self.ci < self.comments.len() {
            let c = &self.comments[self.ci];
            if c.placement == CommentPlacement::EndOfLine && c.span.start < body_start {
                let t = normalize_comment(&c.text);
                let end = c.span.end;
                self.ci += 1;
                self.attach_trailing(&t);
                self.last_end = end;
            }
        }
    }

    fn has_comment_within(&self, a: u32, b: u32) -> bool {
        let mut j = self.ci;
        while j < self.comments.len() && self.comments[j].span.start < b {
            if self.comments[j].span.start > a {
                return true;
            }
            j += 1;
        }
        false
    }

    // --- module + items ---

    fn emit_module(&mut self, m: &Module) {
        for item in &m.items {
            self.advance_to(item_start(item), 2);
            self.emit_item(item);
        }
        self.flush_comments(u32::MAX, 2);
    }

    fn emit_item(&mut self, item: &Item) {
        match item {
            Item::Function(f) => self.emit_function(f),
            Item::Struct(s) => self.emit_struct(s),
            Item::Interface(i) => self.emit_interface(i),
            Item::Enum(e) => self.emit_enum(e),
            Item::Annotation(a) => self.emit_annotation(a),
            Item::Decorator(d) => self.emit_decorator_def(d),
            Item::Import(imp) => {
                self.push_line(&import_str(imp));
                self.last_end = import_span(imp).end;
                self.take_trailing(import_span(imp).end);
            }
            Item::Stmt(s) => self.render_stmt(s),
            Item::Cfg(c) => self.emit_cfg_item(c),
        }
    }

    /// Emit decorator lines. The caller has already `advance_to`'d the first
    /// decorator's start; subsequent decorators get their own flush.
    fn emit_decorators(&mut self, decos: &[Decorator]) {
        for (i, d) in decos.iter().enumerate() {
            if i > 0 {
                self.advance_to(d.span.start, 1);
            }
            let line = if d.args.is_empty() {
                format!("@{}", d.name)
            } else {
                format!("@{}({})", d.name, render_args(&d.args))
            };
            self.push_line(&line);
            self.last_end = d.span.end;
            self.take_trailing(d.span.end);
        }
    }

    fn emit_function(&mut self, f: &Function) {
        self.emit_decorators(&f.decorators);
        if !f.decorators.is_empty() {
            self.advance_to(f.span.start, 1);
        }
        self.emit_signature("def ", &f.name, &f.generics, &f.params, &f.ret, true);
        self.take_header_trailing(f.body.span.start);
        self.emit_block(&f.body);
    }

    /// Emit a `def`/method signature, wrapping the parameter list if it overflows.
    /// `colon` adds the trailing `:` of a definition (interfaces omit it).
    fn emit_signature(
        &mut self,
        kw: &str,
        name: &str,
        generics: &[TypeParam],
        params: &[Param],
        ret: &Option<Type>,
        colon: bool,
    ) {
        let head = format!("{kw}{name}{}(", generics_str(generics));
        let ret_s = match ret {
            Some(t) => format!(" -> {}", type_str(t)),
            None => String::new(),
        };
        let tail = format!("){ret_s}{}", if colon { ":" } else { "" });
        let params_s = params.iter().map(param_str).collect::<Vec<_>>().join(", ");
        let inline = format!("{head}{params_s}{tail}");
        if self.fits(&inline) || params.is_empty() {
            self.push_line(&inline);
            return;
        }
        self.push_line(&head);
        self.indent += 1;
        for p in params {
            self.push_line(&format!("{},", param_str(p)));
        }
        self.indent -= 1;
        self.push_line(&tail);
    }

    fn emit_struct(&mut self, s: &StructDef) {
        self.emit_decorators(&s.decorators);
        if !s.decorators.is_empty() {
            self.advance_to(s.span.start, 1);
        }
        let kw = if s.is_value { "value struct" } else { "struct" };
        self.push_line(&format!("{kw} {}{}:", s.name, generics_str(&s.generics)));
        let mut members: Vec<Member> = Vec::new();
        members.extend(s.fields.iter().map(Member::Field));
        members.extend(s.methods.iter().map(Member::Method));
        members.sort_by_key(|m| m.start());
        self.emit_members(&members, s.span.end);
    }

    fn emit_enum(&mut self, e: &EnumDef) {
        self.emit_decorators(&e.decorators);
        if !e.decorators.is_empty() {
            self.advance_to(e.span.start, 1);
        }
        self.push_line(&format!("enum {}{}:", e.name, generics_str(&e.generics)));
        let mut members: Vec<Member> = Vec::new();
        members.extend(e.variants.iter().map(Member::Variant));
        members.extend(e.methods.iter().map(Member::Method));
        members.sort_by_key(|m| m.start());
        self.emit_members(&members, e.span.end);
    }

    fn emit_interface(&mut self, i: &InterfaceDef) {
        self.push_line(&format!(
            "interface {}{}:",
            i.name,
            generics_str(&i.generics)
        ));
        self.indent += 1;
        self.block_fresh = true;
        for m in &i.methods {
            self.advance_to(m.span.start, 1);
            self.emit_signature("def ", &m.name, &[], &m.params, &m.ret, false);
            self.last_end = m.span.end;
            self.take_trailing(m.span.end);
        }
        self.flush_dangling(i.span.end);
        self.indent -= 1;
        self.block_fresh = false;
    }

    fn emit_annotation(&mut self, a: &AnnotationDef) {
        self.push_line(&format!("annotation {}:", a.name));
        let members: Vec<Member> = a.fields.iter().map(Member::Field).collect();
        self.emit_members(&members, a.span.end);
    }

    fn emit_decorator_def(&mut self, d: &DecoratorDef) {
        let params = if d.params.is_empty() {
            String::new()
        } else {
            format!(
                "({})",
                d.params
                    .iter()
                    .map(param_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        self.push_line(&format!("decorator {}{params}:", d.name));
        self.take_header_trailing(d.body.span.start);
        self.emit_block(&d.body);
    }

    /// Emit the members of a struct/enum/annotation body in source order.
    fn emit_members(&mut self, members: &[Member], body_end: u32) {
        self.indent += 1;
        self.block_fresh = true;
        for m in members {
            self.advance_to(m.start(), 1);
            match m {
                Member::Field(f) => self.emit_field(f),
                Member::Variant(v) => {
                    self.push_line(&variant_str(v));
                    self.last_end = v.span.end;
                    self.take_trailing(v.span.end);
                }
                Member::Method(f) => self.emit_function(f),
            }
        }
        self.flush_dangling(body_end);
        self.indent -= 1;
        self.block_fresh = false;
    }

    fn emit_field(&mut self, f: &Field) {
        self.emit_decorators(&f.decorators);
        if !f.decorators.is_empty() {
            self.advance_to(f.span.start, 1);
        }
        self.push_line(&field_str(f));
        self.last_end = f.span.end;
        self.take_trailing(f.span.end);
    }

    // --- blocks + statements ---

    fn emit_block(&mut self, block: &Block) {
        self.indent += 1;
        self.block_fresh = true;
        for s in &block.stmts {
            self.advance_to(stmt_span(s).start, 1);
            self.render_stmt(s);
        }
        self.flush_dangling(block.span.end);
        self.indent -= 1;
        self.block_fresh = false;
    }

    fn render_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let {
                is_const,
                ty,
                name,
                value,
                span,
            } => {
                let prefix = format!("{}{} = ", let_lead(*is_const, ty), name);
                self.emit_value(&prefix, value, "");
                self.take_trailing(span.end);
                self.last_end = span.end;
            }
            Stmt::LetTuple {
                is_const,
                ty,
                target,
                value,
                span,
            } => {
                let prefix = format!(
                    "{}{} = ",
                    let_lead(*is_const, ty),
                    bind_target_str(target, true)
                );
                self.emit_value(&prefix, value, "");
                self.take_trailing(span.end);
                self.last_end = span.end;
            }
            Stmt::Assign {
                target,
                value,
                span,
            } => {
                // `x ~= s/a/b/` is parsed as an assignment of a self-substitution;
                // render it back in the compact in-place form.
                if let Expr::RegexSubst {
                    subject,
                    pattern,
                    replacement,
                    flags,
                    ..
                } = value
                {
                    if render(subject) == render(target) {
                        self.push_line(&format!(
                            "{} ~= {}",
                            render(target),
                            regex_subst_lit(pattern, replacement, flags)
                        ));
                        self.take_trailing(span.end);
                        self.last_end = span.end;
                        return;
                    }
                }
                let prefix = format!("{} = ", render(target));
                self.emit_value(&prefix, value, "");
                self.take_trailing(span.end);
                self.last_end = span.end;
            }
            Stmt::AssignTuple {
                target,
                value,
                span,
            } => {
                let prefix = format!("{} = ", bind_target_str(target, true));
                self.emit_value(&prefix, value, "");
                self.take_trailing(span.end);
                self.last_end = span.end;
            }
            Stmt::AugAssign {
                target,
                op,
                value,
                span,
            } => {
                let prefix = format!("{} {}= ", render(target), binop_str(*op));
                self.emit_value(&prefix, value, "");
                self.take_trailing(span.end);
                self.last_end = span.end;
            }
            Stmt::Expr(e) => {
                self.emit_value("", e, "");
                self.take_trailing(espan(e).end);
                self.last_end = espan(e).end;
            }
            Stmt::Return { value, span } => {
                match value {
                    Some(v) => self.emit_value("return ", v, ""),
                    None => self.push_line("return"),
                }
                self.take_trailing(span.end);
                self.last_end = span.end;
            }
            Stmt::Raise { value, span } => {
                match value {
                    Some(v) => self.emit_value("raise ", v, ""),
                    None => self.push_line("raise"),
                }
                self.take_trailing(span.end);
                self.last_end = span.end;
            }
            Stmt::Assert {
                cond, msg, span, ..
            } => {
                let suffix = match msg {
                    Some(m) => format!(", {}", pexpr(m, 0)),
                    None => String::new(),
                };
                self.emit_value("assert ", cond, &suffix);
                self.take_trailing(span.end);
                self.last_end = span.end;
            }
            Stmt::Spawn {
                call,
                detached,
                span,
            } => {
                let prefix = if *detached {
                    "spawn detached "
                } else {
                    "spawn "
                };
                self.emit_value(prefix, call, "");
                self.take_trailing(span.end);
                self.last_end = span.end;
            }
            Stmt::Break(span) => {
                self.push_line("break");
                self.take_trailing(span.end);
                self.last_end = span.end;
            }
            Stmt::Continue(span) => {
                self.push_line("continue");
                self.take_trailing(span.end);
                self.last_end = span.end;
            }
            Stmt::If {
                branches,
                else_block,
                ..
            } => self.emit_if(branches, else_block),
            Stmt::While { cond, body, .. } => {
                self.push_line(&format!("while {}:", pexpr(cond, 0)));
                self.take_header_trailing(body.span.start);
                self.emit_block(body);
            }
            Stmt::DoWhile { body, cond, span } => {
                self.push_line("do:");
                self.take_header_trailing(body.span.start);
                self.emit_block(body);
                self.push_line(&format!("while {}", pexpr(cond, 0)));
                self.last_end = span.end;
            }
            Stmt::For {
                target, iter, body, ..
            } => {
                self.push_line(&format!(
                    "for {} in {}:",
                    for_target_str(target),
                    pexpr(iter, 0)
                ));
                self.take_header_trailing(body.span.start);
                self.emit_block(body);
            }
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => self.emit_match(scrutinee, arms, span.end),
            Stmt::Try {
                body,
                excepts,
                finally,
                ..
            } => {
                self.emit_try(body, excepts, finally);
            }
            Stmt::With { items, body, .. } => {
                let items_s = items
                    .iter()
                    .map(|it| match &it.alias {
                        Some(a) => format!("{} as {a}", pexpr(&it.expr, 0)),
                        None => pexpr(&it.expr, 0),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                self.push_line(&format!("with {items_s}:"));
                self.take_header_trailing(body.span.start);
                self.emit_block(body);
            }
            Stmt::Scope { opts, body, .. } => {
                let head = if opts.is_empty() {
                    "scope:".to_string()
                } else {
                    format!("scope({}):", render_args(opts))
                };
                self.push_line(&head);
                self.take_header_trailing(body.span.start);
                self.emit_block(body);
            }
            Stmt::Cfg(c) => self.emit_cfg_stmt(c),
        }
    }

    /// Render a statement-position `when target.*` block (I#139, D-0079): one
    /// `when`/`elif` line per arm plus an optional `else`, each over a statement
    /// block. Mirrors [`Self::emit_if`].
    fn emit_cfg_stmt(&mut self, c: &CfgStmtBlock) {
        for (i, arm) in c.arms.iter().enumerate() {
            let kw = if i == 0 { "when" } else { "elif" };
            if i > 0 {
                self.advance_to(arm.span.start, 1);
            }
            self.push_line(&format!("{kw} {}:", cfg_predicate_str(&arm.predicate)));
            self.take_header_trailing(arm.body.span.start);
            self.emit_block(&arm.body);
        }
        if let Some(eb) = &c.else_body {
            if let Some(es) = c.else_span {
                self.advance_to(es.start, 1);
            }
            self.push_line("else:");
            self.take_header_trailing(eb.span.start);
            self.emit_block(eb);
        }
    }

    /// Render an item-position `when target.*` block (I#139, D-0079): one
    /// `when`/`elif` line per arm plus an optional `else`, each over a block of
    /// items (imports/declarations/bare statements).
    fn emit_cfg_item(&mut self, c: &CfgItemBlock) {
        for (i, arm) in c.arms.iter().enumerate() {
            let kw = if i == 0 { "when" } else { "elif" };
            if i > 0 {
                self.advance_to(arm.span.start, 1);
            }
            self.push_line(&format!("{kw} {}:", cfg_predicate_str(&arm.predicate)));
            let body_start = arm.body.first().map_or(arm.span.end, item_start);
            self.take_header_trailing(body_start);
            self.emit_item_block(&arm.body, arm.span.end);
        }
        if let Some(eb) = &c.else_body {
            if let Some(es) = c.else_span {
                self.advance_to(es.start, 1);
            }
            self.push_line("else:");
            let body_start = eb.first().map_or(c.span.end, item_start);
            self.take_header_trailing(body_start);
            self.emit_item_block(eb, c.span.end);
        }
    }

    /// Emit an indented block of items (the body of an item-position `when`
    /// arm). The item counterpart of [`Self::emit_block`].
    fn emit_item_block(&mut self, items: &[Item], end: u32) {
        self.indent += 1;
        self.block_fresh = true;
        for it in items {
            self.advance_to(item_start(it), 2);
            self.emit_item(it);
        }
        self.flush_dangling(end);
        self.indent -= 1;
        self.block_fresh = false;
    }

    fn emit_if(&mut self, branches: &[(Expr, Block)], else_block: &Option<Block>) {
        for (i, (cond, body)) in branches.iter().enumerate() {
            let kw = if i == 0 { "if" } else { "elif" };
            if i > 0 {
                self.advance_to(espan(cond).start, 1);
            }
            self.push_line(&format!("{kw} {}:", pexpr(cond, 0)));
            self.take_header_trailing(body.span.start);
            self.emit_block(body);
        }
        if let Some(eb) = else_block {
            self.advance_to(eb.span.start, 1);
            self.push_line("else:");
            self.take_header_trailing(eb.span.start);
            self.emit_block(eb);
        }
    }

    fn emit_try(&mut self, body: &Block, excepts: &[ExceptClause], finally: &Option<Block>) {
        self.push_line("try:");
        self.take_header_trailing(body.span.start);
        self.emit_block(body);
        for ex in excepts {
            self.advance_to(ex.span.start, 1);
            let mut head = "except".to_string();
            if let Some(t) = &ex.ty {
                head.push(' ');
                head.push_str(&type_str(t));
            }
            if let Some(b) = &ex.binding {
                head.push_str(" as ");
                head.push_str(b);
            }
            head.push(':');
            self.push_line(&head);
            self.take_header_trailing(ex.body.span.start);
            self.emit_block(&ex.body);
        }
        if let Some(fin) = finally {
            self.advance_to(fin.span.start, 1);
            self.push_line("finally:");
            self.take_header_trailing(fin.span.start);
            self.emit_block(fin);
        }
    }

    fn emit_match(&mut self, scrutinee: &Expr, arms: &[MatchArm], body_end: u32) {
        self.push_line(&format!("match {}:", pexpr(scrutinee, 0)));
        let first = arms.first().map_or(body_end, |a| a.span.start);
        self.take_header_trailing(first);
        self.indent += 1;
        self.block_fresh = true;
        for arm in arms {
            self.advance_to(arm.span.start, 1);
            let guard = match &arm.guard {
                Some(g) => format!(" if {}", pexpr(g, 0)),
                None => String::new(),
            };
            self.push_line(&format!("case {}{guard}:", pattern_str(&arm.pattern)));
            self.take_header_trailing(arm.body.span.start);
            self.emit_block(&arm.body);
        }
        self.flush_dangling(body_end);
        self.indent -= 1;
        self.block_fresh = false;
    }

    // --- expression emission with width-triggered wrapping ---

    /// Emit `prefix + expr + suffix` on one line if it fits and holds no interior
    /// comments; otherwise break the expression across lines.
    fn emit_value(&mut self, prefix: &str, e: &Expr, suffix: &str) {
        let inline = format!("{prefix}{}{suffix}", render(e));
        let sp = espan(e);
        if self.fits(&inline) && !self.has_comment_within(sp.start, sp.end) {
            self.push_line(&inline);
            return;
        }
        if self.break_expr(prefix, e, suffix) {
            return;
        }
        self.push_line(&inline);
    }

    /// Break a bracketed expression across lines (one element per line, trailing
    /// comma), recursing into elements and preserving interior comments. Returns
    /// `false` when `e` is not a breakable shape.
    fn break_expr(&mut self, prefix: &str, e: &Expr, suffix: &str) -> bool {
        let sp = espan(e);
        // An empty container is only broken to keep an interior comment inside
        // its brackets — never just because the line is long.
        let keep = |nonempty: bool, p: &Self| nonempty || p.has_comment_within(sp.start, sp.end);
        match e {
            Expr::Call { callee, args, .. } if keep(!args.is_empty(), self) => {
                let head = format!("{prefix}{}(", pexpr(callee, 15));
                self.push_line(&head);
                self.indent += 1;
                for (i, a) in args.iter().enumerate() {
                    self.flush_comments(a.span.start, 0);
                    let ap = match &a.name {
                        Some(n) => format!("{n}="),
                        None => String::new(),
                    };
                    self.emit_value(&ap, &a.value, ",");
                    let before = args.get(i + 1).map_or(sp.end, |n| n.span.start);
                    self.take_trailing_before(espan(&a.value).end, before);
                }
                self.flush_comments(sp.end, 0);
                self.indent -= 1;
                self.push_line(&format!("){suffix}"));
                true
            }
            Expr::List(xs, _) if keep(!xs.is_empty(), self) => {
                self.break_seq(prefix, "[", xs, "]", suffix, sp.end)
            }
            Expr::Set(xs, _) if keep(!xs.is_empty(), self) => {
                self.break_seq(prefix, "{", xs, "}", suffix, sp.end)
            }
            Expr::Tuple(xs, _) if keep(!xs.is_empty(), self) => {
                self.break_seq(prefix, "(", xs, ")", suffix, sp.end)
            }
            Expr::Dict(pairs, _) if keep(!pairs.is_empty(), self) => {
                self.push_line(&format!("{prefix}{{"));
                self.indent += 1;
                for (i, (k, v)) in pairs.iter().enumerate() {
                    self.flush_comments(espan(k).start, 0);
                    self.emit_value(&format!("{}: ", pexpr(k, 0)), v, ",");
                    let before = pairs.get(i + 1).map_or(sp.end, |(nk, _)| espan(nk).start);
                    self.take_trailing_before(espan(v).end, before);
                }
                self.flush_comments(sp.end, 0);
                self.indent -= 1;
                self.push_line(&format!("}}{suffix}"));
                true
            }
            _ => false,
        }
    }

    fn break_seq(
        &mut self,
        prefix: &str,
        open: &str,
        xs: &[Expr],
        close: &str,
        suffix: &str,
        end: u32,
    ) -> bool {
        self.push_line(&format!("{prefix}{open}"));
        self.indent += 1;
        for (i, x) in xs.iter().enumerate() {
            self.flush_comments(espan(x).start, 0);
            self.emit_value("", x, ",");
            let before = xs.get(i + 1).map_or(end, |n| espan(n).start);
            self.take_trailing_before(espan(x).end, before);
        }
        self.flush_comments(end, 0);
        self.indent -= 1;
        self.push_line(&format!("{close}{suffix}"));
        true
    }
}

// --- lead/field/variant/import helpers (declarations) ---

fn let_lead(is_const: bool, ty: &Option<Type>) -> String {
    let c = if is_const { "const " } else { "" };
    match ty {
        Some(t) => format!("{c}{} ", type_str(t)),
        None => format!("{c}auto "),
    }
}

fn field_str(f: &Field) -> String {
    let mut s = format!("{} {}", type_str(&f.ty), f.name);
    if let Some(bits) = f.bits {
        s.push_str(&format!(": {bits}"));
    }
    if let Some(d) = &f.default {
        s.push_str(&format!(" = {}", pexpr(d, 0)));
    }
    s
}

fn variant_str(v: &EnumVariant) -> String {
    if !v.fields.is_empty() {
        let payload = v
            .fields
            .iter()
            .map(|vf| format!("{} {}", type_str(&vf.ty), vf.name))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}({payload})", v.name)
    } else if let Some(val) = &v.value {
        format!("{} = {}", v.name, pexpr(val, 0))
    } else {
        v.name.clone()
    }
}

fn import_str(imp: &Import) -> String {
    match imp {
        Import::Module { path, alias, .. } => match alias {
            Some(a) => format!("import {path} as {a}"),
            None => format!("import {path}"),
        },
        Import::Python { module, alias, .. } => {
            let m = format_string(module, false, false, false);
            match alias {
                Some(a) => format!("import python {m} as {a}"),
                None => format!("import python {m}"),
            }
        }
    }
}

fn import_span(imp: &Import) -> Span {
    match imp {
        Import::Module { span, .. } | Import::Python { span, .. } => *span,
    }
}

/// Render a `when target.*` predicate back to source (I#139, D-0079).
fn cfg_predicate_str(p: &CfgPredicate) -> String {
    match p {
        CfgPredicate::Arch(a) => format!("target.arch == .{}", a.as_str()),
        CfgPredicate::Os(o) => format!("target.os == .{}", o.as_str()),
        CfgPredicate::HasFeature(f) => format!("target.has(.{f})"),
    }
}

fn item_start(item: &Item) -> u32 {
    match item {
        Item::Function(f) => f.decorators.first().map_or(f.span.start, |d| d.span.start),
        Item::Struct(s) => s.decorators.first().map_or(s.span.start, |d| d.span.start),
        Item::Enum(e) => e.decorators.first().map_or(e.span.start, |d| d.span.start),
        Item::Interface(i) => i.span.start,
        Item::Annotation(a) => a.span.start,
        Item::Decorator(d) => d.span.start,
        Item::Import(imp) => import_span(imp).start,
        Item::Stmt(s) => stmt_span(s).start,
        Item::Cfg(c) => c.span.start,
    }
}

fn stmt_span(s: &Stmt) -> Span {
    match s {
        Stmt::Let { span, .. }
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
        | Stmt::Scope { span, .. }
        | Stmt::LetTuple { span, .. }
        | Stmt::AssignTuple { span, .. }
        | Stmt::Cfg(CfgStmtBlock { span, .. }) => *span,
        Stmt::Expr(e) => espan(e),
        Stmt::Break(span) | Stmt::Continue(span) => *span,
    }
}

/// The source span of an expression (every variant carries one).
fn espan(e: &Expr) -> Span {
    match e {
        Expr::Int(_, s)
        | Expr::Float(_, s)
        | Expr::Imaginary(_, s)
        | Expr::Bool(_, s)
        | Expr::None(s)
        | Expr::Name(_, s)
        | Expr::Tuple(_, s)
        | Expr::List(_, s)
        | Expr::Set(_, s)
        | Expr::Dict(_, s) => *s,
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
