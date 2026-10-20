//! Recursive-descent parser with a precedence-climbing ("Pratt") expression
//! parser (Phase 1).
//!
//! Consumes the token stream from [`crate::lexer`] (including the synthesized
//! `Newline`/`Indent`/`Dedent` layout tokens) and produces the [`crate::ast`].
//! Errors are collected rather than aborting, with best-effort recovery to the
//! next line.
//!
//! Precedence (low → high): lambda · `if`/`else` · `or` · `and` · `not` ·
//! comparison / `in` / `=~` / `!~` · `|` · `^` · `&` · `<<`/`>>` · `+`/`-` ·
//! `*`/`/`/`//`/`%` · unary `+`/`-`/`~` · `**` · call/index/slice/attr.

use crate::ast::*;
use crate::lexer::tokenize;
use crate::span::Span;
use crate::token::{Keyword, Token, TokenKind};

/// A parse error with the offending source span.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

/// Parse a token stream into a module plus any errors.
pub fn parse(tokens: Vec<Token>) -> (Module, Vec<ParseError>) {
    parse_with_source(tokens, "")
}

/// Like [`parse`], but with the original source so nodes that need to echo the
/// exact source text (e.g. `assert` for power-assert diagnostics, D-0054) can
/// slice it by span.
pub fn parse_with_source(tokens: Vec<Token>, src: &str) -> (Module, Vec<ParseError>) {
    let mut p = Parser::new(tokens, src);
    let module = p.parse_module();
    (module, p.errors)
}

/// Convenience: lex and parse `src`. Lex errors are returned alongside parse
/// errors.
pub fn parse_str(src: &str) -> (Module, Vec<ParseError>, Vec<crate::lexer::LexError>) {
    let (tokens, lex_errors) = tokenize(src);
    let (module, parse_errors) = parse_with_source(tokens, src);
    (module, parse_errors, lex_errors)
}

/// The compiler-known built-in prelude (ticket #77, D-0058; ticket #99): the
/// `Signal` enum and `MemStats`/`DateTime` structs that back the `signal`/`gc`/
/// `runtime`/`datetime` surface. They are injected ahead of the user's items
/// (see [`prelude_items`]) so every ordinary enum/struct path — member access,
/// `match`, `channel[Signal]`, field access — works unchanged. A `Signal`
/// variant's backing integer IS its Unix (Linux) signal number, so the runtime
/// ABI speaks raw signal numbers. SIGKILL/SIGSTOP are intentionally absent
/// (uncatchable). `MemStats` mirrors `gc.stats()`; `DateTime` is the civil-time
/// value `datetime.*` produces (its methods live on the module, so the struct
/// itself stays method-free and emits no code when a program never uses it).
/// `JsonValue` is the dynamic JSON DOM sum type (D-0059, I#100/I#131): a
/// data-carrying enum (D-0046) with `Int(int64)`/`Float(float64)` kept distinct,
/// and recursive `Array(list[JsonValue])` / `Object(dict[str, JsonValue])`
/// variants; `json.loads`/`dumps` and `@derive(Json)` are built on it.
pub const PRELUDE_SRC: &str = "\
enum Signal:
    SIGHUP = 1
    SIGINT = 2
    SIGQUIT = 3
    SIGABRT = 6
    SIGUSR1 = 10
    SIGUSR2 = 12
    SIGPIPE = 13
    SIGALRM = 14
    SIGTERM = 15
    SIGCHLD = 17
    SIGCONT = 18
    SIGTSTP = 20
    SIGWINCH = 28

value struct MemStats:
    int64 heap_bytes
    int64 live_bytes
    int64 free_bytes
    int64 gc_count
    int64 goroutines

value struct DateTime:
    int64 year
    int64 month
    int64 day
    int64 hour
    int64 minute
    int64 second

enum JsonValue:
    Null
    Bool(bool b)
    Int(int64 i)
    Float(float64 f)
    Str(str s)
    Array(list[JsonValue] items)
    Object(dict[str, JsonValue] fields)

def __json_value(bytes c, int64 k) -> JsonValue:
    if k == 1:
        return JsonValue.Null
    if k == 2:
        return JsonValue.Bool(True)
    if k == 3:
        return JsonValue.Bool(False)
    if k == 4:
        return JsonValue.Int(json._lex_int(c))
    if k == 5:
        return JsonValue.Float(json._lex_float(c))
    if k == 6:
        return JsonValue.Str(json._lex_str(c))
    if k == 9:
        return __json_array(c)
    if k == 7:
        return __json_object(c)
    return JsonValue.Null

def __json_array(bytes c) -> JsonValue:
    list[JsonValue] xs = []
    auto k = json._lex_next(c)
    while k != 10:
        xs.append(__json_value(c, k))
        k = json._lex_next(c)
        if k == 12:
            k = json._lex_next(c)
    return JsonValue.Array(xs)

def __json_object(bytes c) -> JsonValue:
    dict[str, JsonValue] m = {}
    auto k = json._lex_next(c)
    while k == 6:
        auto key = json._lex_str(c)
        json._lex_next(c)
        auto vk = json._lex_next(c)
        m[key] = __json_value(c, vk)
        k = json._lex_next(c)
        if k == 12:
            k = json._lex_next(c)
    return JsonValue.Object(m)

def __json_loads(str s) -> JsonValue:
    auto c = json._lex_new(s)
    auto k = json._lex_next(c)
    return __json_value(c, k)

def __json_dump(JsonValue v, list[str] out):
    match v:
        case JsonValue.Null:
            out.append(\"null\")
        case JsonValue.Bool(b):
            if b:
                out.append(\"true\")
            else:
                out.append(\"false\")
        case JsonValue.Int(i):
            out.append(json._i64_str(i))
        case JsonValue.Float(f):
            out.append(json._f64_str(f))
        case JsonValue.Str(s):
            out.append(json._escape(s))
        case JsonValue.Array(items):
            out.append(\"[\")
            auto first = True
            for x in items:
                if first:
                    first = False
                else:
                    out.append(\",\")
                __json_dump(x, out)
            out.append(\"]\")
        case JsonValue.Object(fields):
            out.append(\"{\")
            auto first = True
            list[str] ks = json._obj_keys(fields)
            for key in ks:
                if first:
                    first = False
                else:
                    out.append(\",\")
                out.append(json._escape(key))
                out.append(\":\")
                __json_dump(fields[key], out)
            out.append(\"}\")

def __json_dumps(JsonValue v) -> str:
    list[str] parts = []
    __json_dump(v, parts)
    return json._join(parts)

def __json_as_int(JsonValue v) -> int64:
    match v:
        case JsonValue.Int(i):
            return i
        case _:
            return 0

def __json_as_float(JsonValue v) -> float64:
    match v:
        case JsonValue.Float(f):
            return f
        case JsonValue.Int(i):
            return 0.0
        case _:
            return 0.0

def __json_as_bool(JsonValue v) -> bool:
    match v:
        case JsonValue.Bool(b):
            return b
        case _:
            return False

def __json_as_str(JsonValue v) -> str:
    match v:
        case JsonValue.Str(s):
            return s
        case _:
            return \"\"

def __json_field(JsonValue v, str k) -> JsonValue:
    match v:
        case JsonValue.Object(m):
            return m[k]
        case _:
            return JsonValue.Null
";

/// Parse the built-in [`PRELUDE_SRC`] into AST items. The prelude is authored to
/// lex and parse cleanly; a failure here is a compiler bug. Callers (sema and
/// the lowerer) prepend these ahead of the user's items, skipping any whose name
/// the user has already defined (so a program may still declare its own type of
/// that name without a duplicate-definition error).
pub fn prelude_items() -> Vec<crate::ast::Item> {
    let (module, perrs, lerrs) = parse_str(PRELUDE_SRC);
    debug_assert!(
        perrs.is_empty() && lerrs.is_empty(),
        "built-in prelude must parse cleanly: {perrs:?} {lerrs:?}"
    );
    module.items
}

/// The declared type name of a top-level item, if it introduces one.
fn item_type_name(item: &crate::ast::Item) -> Option<&str> {
    use crate::ast::Item;
    match item {
        Item::Struct(s) => Some(&s.name),
        Item::Enum(e) => Some(&e.name),
        Item::Interface(i) => Some(&i.name),
        Item::Annotation(a) => Some(&a.name),
        _ => None,
    }
}

/// Does `module` reference any of the signals surface (ticket #77)? Used to
/// gate prelude injection so programs that never touch signals are byte-for-byte
/// unchanged (no extra `Signal`/`MemStats` type in the lowered module). The scan
/// is over the derived `Debug` of the AST with QUOTED needles, so each needle
/// matches a whole string-valued field (an identifier, type name, decorator name
/// or scope-option key) — `"gc"` matches `Name("gc")` but not `Name("gc_total")`.
/// A rare false positive (e.g. a string literal `"gc"`) only injects two unused
/// declarations, which emit no code; it never changes observable behaviour.
fn references_prelude(module: &Module) -> bool {
    const NEEDLES: &[&str] = &[
        "\"Signal\"",
        "\"MemStats\"",
        "\"signal\"",
        "\"gc\"",
        "\"runtime\"",
        "\"on_signal\"",
        "\"cancel_on\"",
        "\"grace\"",
        "\"DateTime\"",
        "\"time\"",
        "\"datetime\"",
        "\"JsonValue\"",
        "\"json\"",
        "\"Json\"",
    ];
    let dbg = format!("{module:?}");
    NEEDLES.iter().any(|n| dbg.contains(n))
}

/// Return `module` augmented with the built-in [`prelude_items`] when it uses the
/// signals surface (see [`references_prelude`]); otherwise return a plain clone.
/// A prelude item is skipped if the user already declares a type of that name, so
/// user code can still shadow `Signal`/`MemStats` without a duplicate-definition
/// error. Both the type checker and the lowerer apply this, so they agree on the
/// surface a program sees (ticket #77, D-0058).
pub fn with_prelude(module: &Module) -> Module {
    if !references_prelude(module) {
        return module.clone();
    }
    let user_names: std::collections::HashSet<&str> =
        module.items.iter().filter_map(item_type_name).collect();
    let mut items: Vec<crate::ast::Item> = prelude_items()
        .into_iter()
        .filter(|it| match item_type_name(it) {
            Some(n) => !user_names.contains(n),
            None => true,
        })
        .collect();
    items.extend(module.items.iter().cloned());
    Module {
        items,
        span: module.span,
    }
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    errors: Vec<ParseError>,
    /// The original source, kept so a node can echo its exact spelling by span
    /// (power-assert, D-0054). Empty when the source was not supplied.
    src: String,
}

impl Parser {
    fn new(mut toks: Vec<Token>, src: &str) -> Parser {
        if toks.last().map(|t| &t.kind) != Some(&TokenKind::Eof) {
            let end = toks.last().map(|t| t.span.end).unwrap_or(0);
            toks.push(Token::new(TokenKind::Eof, Span::empty_at(end)));
        }
        Parser {
            toks,
            pos: 0,
            errors: Vec::new(),
            src: src.to_string(),
        }
    }

    /// The exact source text covered by `span`, or `""` when the source was not
    /// provided or the span is out of range.
    fn source_slice(&self, span: Span) -> &str {
        let (lo, hi) = (span.start as usize, span.end as usize);
        if lo <= hi
            && hi <= self.src.len()
            && self.src.is_char_boundary(lo)
            && self.src.is_char_boundary(hi)
        {
            &self.src[lo..hi]
        } else {
            ""
        }
    }

    // --- cursor ---

    fn peek(&self) -> &TokenKind {
        &self.toks[self.pos].kind
    }

    fn peek2(&self) -> &TokenKind {
        self.toks
            .get(self.pos + 1)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    fn peek_span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn prev_end(&self) -> u32 {
        if self.pos == 0 {
            0
        } else {
            self.toks[self.pos - 1].span.end
        }
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if !self.at_eof() {
            self.pos += 1;
        }
        t
    }

    fn is(&self, k: &TokenKind) -> bool {
        self.peek() == k
    }

    fn is_kw(&self, kw: Keyword) -> bool {
        matches!(self.peek(), TokenKind::Keyword(k) if *k == kw)
    }

    fn eat(&mut self, k: &TokenKind) -> bool {
        if self.is(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, kw: Keyword) -> bool {
        if self.is_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, k: &TokenKind, what: &str) {
        if !self.eat(k) {
            self.error_here(format!("expected {what}"));
        }
    }

    fn expect_kw(&mut self, kw: Keyword, what: &str) {
        if !self.eat_kw(kw) {
            self.error_here(format!("expected {what}"));
        }
    }

    fn error_here(&mut self, message: impl Into<String>) {
        self.errors.push(ParseError {
            message: message.into(),
            span: self.peek_span(),
        });
    }

    fn span_from(&self, start: u32) -> Span {
        Span::new(start, self.prev_end())
    }

    fn ident_opt(&mut self) -> Option<(String, Span)> {
        if let TokenKind::Ident(name) = self.peek() {
            let name = name.clone();
            let span = self.peek_span();
            self.bump();
            Some((name, span))
        } else {
            None
        }
    }

    fn ident(&mut self, what: &str) -> String {
        match self.ident_opt() {
            Some((name, _)) => name,
            None => {
                self.error_here(format!("expected {what}"));
                "<error>".to_string()
            }
        }
    }

    fn skip_newlines(&mut self) {
        while self.is(&TokenKind::Newline) {
            self.bump();
        }
    }

    // --- module / items ---

    fn parse_module(&mut self) -> Module {
        let start = self.peek_span().start;
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at_eof() {
            let before = self.pos;
            self.parse_top_level(&mut items);
            self.skip_newlines();
            if self.pos == before {
                // No progress: skip a token to avoid an infinite loop.
                self.error_here("unexpected token at top level");
                self.bump();
                self.skip_newlines();
            }
        }
        Module {
            items,
            span: self.span_from(start),
        }
    }

    fn parse_top_level(&mut self, items: &mut Vec<Item>) {
        let decos = self.parse_decorators();
        match self.peek() {
            TokenKind::Keyword(Keyword::Def) => {
                items.push(Item::Function(self.parse_function(decos)))
            }
            TokenKind::Keyword(Keyword::Struct) => {
                items.push(Item::Struct(self.parse_struct(decos, false)))
            }
            // Contextual `value struct` modifier (D-0051): `value` is an
            // identifier, not a keyword, so only treat it as the value-type
            // modifier when it immediately precedes `struct`.
            TokenKind::Ident(n)
                if n == "value" && matches!(self.peek2(), TokenKind::Keyword(Keyword::Struct)) =>
            {
                self.bump(); // consume `value`
                items.push(Item::Struct(self.parse_struct(decos, true)))
            }
            TokenKind::Keyword(Keyword::Enum) => items.push(Item::Enum(self.parse_enum(decos))),
            TokenKind::Keyword(Keyword::Interface) => {
                items.push(Item::Interface(self.parse_interface()))
            }
            TokenKind::Keyword(Keyword::Annotation) => {
                items.push(Item::Annotation(self.parse_annotation()))
            }
            TokenKind::Keyword(Keyword::Decorator) => {
                items.push(Item::Decorator(self.parse_decorator_def()))
            }
            TokenKind::Keyword(Keyword::Import) => items.push(Item::Import(self.parse_import())),
            _ => {
                if !decos.is_empty() {
                    self.error_here("decorators must precede a declaration");
                }
                let mut stmts = Vec::new();
                self.parse_line(&mut stmts);
                items.extend(stmts.into_iter().map(Item::Stmt));
            }
        }
    }

    fn parse_decorators(&mut self) -> Vec<Decorator> {
        let mut v = Vec::new();
        while self.is(&TokenKind::At) {
            let start = self.peek_span().start;
            self.bump();
            let name = self.dotted_name();
            let args = if self.eat(&TokenKind::LParen) {
                self.parse_args()
            } else {
                Vec::new()
            };
            self.expect(&TokenKind::Newline, "newline after decorator");
            v.push(Decorator {
                name,
                args,
                span: self.span_from(start),
            });
        }
        v
    }

    fn dotted_name(&mut self) -> String {
        let mut name = self.ident("name");
        while self.is(&TokenKind::Dot) {
            self.bump();
            name.push('.');
            name.push_str(&self.ident("name"));
        }
        name
    }

    fn parse_generics(&mut self) -> Vec<String> {
        let mut v = Vec::new();
        if self.eat(&TokenKind::LBracket) {
            if !self.is(&TokenKind::RBracket) {
                loop {
                    v.push(self.ident("type parameter"));
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RBracket, "']'");
        }
        v
    }

    fn parse_function(&mut self, decorators: Vec<Decorator>) -> Function {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Def, "'def'");
        let name = self.ident("function name");
        let generics = self.parse_generics();
        self.expect(&TokenKind::LParen, "'('");
        let params = self.parse_params();
        self.expect(&TokenKind::RParen, "')'");
        let ret = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type())
        } else {
            None
        };
        let body = self.parse_block();
        Function {
            decorators,
            name,
            generics,
            params,
            ret,
            body,
            span: self.span_from(start),
        }
    }

    fn parse_params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        if self.is(&TokenKind::RParen) {
            return params;
        }
        loop {
            params.push(self.parse_param());
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if self.is(&TokenKind::RParen) {
                break; // trailing comma
            }
        }
        params
    }

    fn parse_param(&mut self) -> Param {
        let start = self.peek_span().start;
        // `self` receiver (untyped).
        if matches!(self.peek(), TokenKind::Ident(n) if n == "self") {
            self.bump();
            return Param {
                decorators: Vec::new(),
                ty: None,
                name: "self".to_string(),
                default: None,
                variadic: false,
                span: self.span_from(start),
            };
        }
        let ty = Some(self.parse_type());
        let variadic = self.eat(&TokenKind::Ellipsis);
        let name = self.ident("parameter name");
        let default = if self.eat(&TokenKind::Assign) {
            Some(self.parse_expr())
        } else {
            None
        };
        Param {
            decorators: Vec::new(),
            ty,
            name,
            default,
            variadic,
            span: self.span_from(start),
        }
    }

    fn parse_struct(&mut self, decorators: Vec<Decorator>, is_value: bool) -> StructDef {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Struct, "'struct'");
        let name = self.ident("struct name");
        let generics = self.parse_generics();
        let (fields, methods) = self.parse_struct_body();
        StructDef {
            decorators,
            name,
            generics,
            fields,
            methods,
            is_value,
            span: self.span_from(start),
        }
    }

    fn parse_struct_body(&mut self) -> (Vec<Field>, Vec<Function>) {
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            let decos = self.parse_decorators();
            if self.is_kw(Keyword::Def) {
                methods.push(self.parse_function(decos));
            } else {
                let fstart = self.peek_span().start;
                let ty = self.parse_type();
                let fname = self.ident("field name");
                let default = if self.eat(&TokenKind::Assign) {
                    Some(self.parse_expr())
                } else {
                    None
                };
                self.expect(&TokenKind::Newline, "newline after field");
                fields.push(Field {
                    decorators: decos,
                    ty,
                    name: fname,
                    default,
                    span: self.span_from(fstart),
                });
            }
            if self.pos == before {
                self.error_here("unexpected token in struct body");
                self.bump();
            }
        }
        self.expect(&TokenKind::Dedent, "end of block");
        (fields, methods)
    }

    fn parse_enum(&mut self, decorators: Vec<Decorator>) -> EnumDef {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Enum, "'enum'");
        let name = self.ident("enum name");
        let generics = self.parse_generics();
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        let mut variants = Vec::new();
        let mut methods = Vec::new();
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            // Methods on the enum (D-0046): a `def` (possibly decorated) in the
            // body, parsed exactly like a struct method.
            let decos = self.parse_decorators();
            if self.is_kw(Keyword::Def) {
                methods.push(self.parse_function(decos));
                if self.pos == before {
                    self.bump();
                }
                continue;
            }
            let vstart = self.peek_span().start;
            let vname = self.ident("variant name");
            // A data-carrying variant: `Circle(f64 r, f64 g, ...)` (D-0046).
            // Type-first named payload fields in parentheses.
            let mut fields = Vec::new();
            let mut value = None;
            if self.eat(&TokenKind::LParen) {
                if !self.is(&TokenKind::RParen) {
                    loop {
                        let fstart = self.peek_span().start;
                        let fty = self.parse_type();
                        let fname = self.ident("payload field name");
                        fields.push(VariantField {
                            ty: fty,
                            name: fname,
                            span: self.span_from(fstart),
                        });
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RParen, "')'");
            } else if self.eat(&TokenKind::Assign) {
                // A tier-1 constant with an explicit discriminant (`GREEN = 10`).
                value = Some(self.parse_expr());
            }
            self.expect(&TokenKind::Newline, "newline after variant");
            variants.push(EnumVariant {
                name: vname,
                value,
                fields,
                span: self.span_from(vstart),
            });
            if self.pos == before {
                self.bump();
            }
        }
        self.expect(&TokenKind::Dedent, "end of block");
        EnumDef {
            decorators,
            name,
            generics,
            variants,
            methods,
            span: self.span_from(start),
        }
    }

    fn parse_interface(&mut self) -> InterfaceDef {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Interface, "'interface'");
        let name = self.ident("interface name");
        let generics = self.parse_generics();
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        let mut methods = Vec::new();
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            let mstart = self.peek_span().start;
            self.expect_kw(Keyword::Def, "'def'");
            let mname = self.ident("method name");
            self.expect(&TokenKind::LParen, "'('");
            let params = self.parse_params();
            self.expect(&TokenKind::RParen, "')'");
            let ret = if self.eat(&TokenKind::Arrow) {
                Some(self.parse_type())
            } else {
                None
            };
            self.expect(&TokenKind::Newline, "newline after method signature");
            methods.push(MethodSig {
                name: mname,
                params,
                ret,
                span: self.span_from(mstart),
            });
            if self.pos == before {
                self.bump();
            }
        }
        self.expect(&TokenKind::Dedent, "end of block");
        InterfaceDef {
            name,
            generics,
            methods,
            span: self.span_from(start),
        }
    }

    fn parse_annotation(&mut self) -> AnnotationDef {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Annotation, "'annotation'");
        let name = self.ident("annotation name");
        let (fields, _methods) = self.parse_struct_body();
        AnnotationDef {
            name,
            fields,
            span: self.span_from(start),
        }
    }

    fn parse_decorator_def(&mut self) -> DecoratorDef {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Decorator, "'decorator'");
        let name = self.ident("decorator name");
        let params = if self.eat(&TokenKind::LParen) {
            let ps = self.parse_params();
            self.expect(&TokenKind::RParen, "')'");
            ps
        } else {
            Vec::new()
        };
        let body = self.parse_block();
        DecoratorDef {
            name,
            params,
            body,
            span: self.span_from(start),
        }
    }

    fn parse_import(&mut self) -> Import {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Import, "'import'");
        if self.eat_kw(Keyword::Python) {
            let module = self.string_lit();
            let alias = if self.eat_kw(Keyword::As) {
                Some(self.ident("alias"))
            } else {
                None
            };
            self.expect(&TokenKind::Newline, "newline");
            Import::Python {
                module,
                alias,
                span: self.span_from(start),
            }
        } else {
            let path = self.dotted_name();
            let alias = if self.eat_kw(Keyword::As) {
                Some(self.ident("alias"))
            } else {
                None
            };
            self.expect(&TokenKind::Newline, "newline");
            Import::Module {
                path,
                alias,
                span: self.span_from(start),
            }
        }
    }

    fn string_lit(&mut self) -> String {
        if let TokenKind::Str { value, .. } = self.peek() {
            let v = value.clone();
            self.bump();
            v
        } else {
            self.error_here("expected a string literal");
            String::new()
        }
    }

    // --- blocks & statements ---

    fn parse_block(&mut self) -> Block {
        let start = self.peek_span().start;
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        let mut stmts = Vec::new();
        while !self.is(&TokenKind::Dedent) && !self.at_eof() {
            let before = self.pos;
            self.parse_line(&mut stmts);
            if self.pos == before {
                self.error_here("unexpected token in block");
                self.bump();
            }
        }
        self.expect(&TokenKind::Dedent, "end of block");
        Block {
            stmts,
            span: self.span_from(start),
        }
    }

    /// Parse one logical line into `out`: a compound statement, or one or more
    /// `;`-separated simple statements terminated by a newline.
    fn parse_line(&mut self, out: &mut Vec<Stmt>) {
        if self.is_compound_start() {
            out.push(self.parse_compound());
            return;
        }
        loop {
            out.push(self.parse_simple_stmt());
            if self.eat(&TokenKind::Semicolon) {
                if self.is(&TokenKind::Newline) || self.is(&TokenKind::Dedent) || self.at_eof() {
                    break;
                }
                continue;
            }
            break;
        }
        if !self.eat(&TokenKind::Newline) && !self.at_eof() && !self.is(&TokenKind::Dedent) {
            self.error_here("expected newline after statement");
        }
    }

    fn is_compound_start(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Keyword(
                Keyword::If
                    | Keyword::While
                    | Keyword::Do
                    | Keyword::For
                    | Keyword::Match
                    | Keyword::Try
                    | Keyword::With
                    | Keyword::Scope
            )
        )
    }

    fn parse_compound(&mut self) -> Stmt {
        match self.peek() {
            TokenKind::Keyword(Keyword::If) => self.parse_if(),
            TokenKind::Keyword(Keyword::While) => self.parse_while(),
            TokenKind::Keyword(Keyword::Do) => self.parse_do_while(),
            TokenKind::Keyword(Keyword::For) => self.parse_for(),
            TokenKind::Keyword(Keyword::Match) => self.parse_match(),
            TokenKind::Keyword(Keyword::Try) => self.parse_try(),
            TokenKind::Keyword(Keyword::With) => self.parse_with(),
            TokenKind::Keyword(Keyword::Scope) => self.parse_scope(),
            _ => unreachable!("parse_compound called on non-compound token"),
        }
    }

    /// `scope [ ( opts ) ]: BLOCK` (D-0044/D-0045). The optional parenthesized
    /// `opts` configure the failure policy / deadline (`on_error=`, `timeout=`);
    /// the block owns every `spawn` lexically inside it.
    fn parse_scope(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Scope, "'scope'");
        let opts = if self.eat(&TokenKind::LParen) {
            self.parse_args()
        } else {
            Vec::new()
        };
        let body = self.parse_block();
        Stmt::Scope {
            opts,
            body,
            span: self.span_from(start),
        }
    }

    fn parse_if(&mut self) -> Stmt {
        let start = self.peek_span().start;
        let mut branches = Vec::new();
        self.expect_kw(Keyword::If, "'if'");
        let cond = self.parse_expr();
        let body = self.parse_block();
        branches.push((cond, body));
        while self.is_kw(Keyword::Elif) {
            self.bump();
            let c = self.parse_expr();
            let b = self.parse_block();
            branches.push((c, b));
        }
        let else_block = if self.eat_kw(Keyword::Else) {
            Some(self.parse_block())
        } else {
            None
        };
        Stmt::If {
            branches,
            else_block,
            span: self.span_from(start),
        }
    }

    fn parse_while(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::While, "'while'");
        let cond = self.parse_expr();
        let body = self.parse_block();
        Stmt::While {
            cond,
            body,
            span: self.span_from(start),
        }
    }

    fn parse_do_while(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Do, "'do'");
        let body = self.parse_block();
        self.expect_kw(Keyword::While, "'while'");
        let cond = self.parse_expr();
        if !self.eat(&TokenKind::Newline) && !self.at_eof() && !self.is(&TokenKind::Dedent) {
            self.error_here("expected newline after 'do'-'while'");
        }
        Stmt::DoWhile {
            body,
            cond,
            span: self.span_from(start),
        }
    }

    fn parse_for(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::For, "'for'");
        let target = self.parse_target();
        self.expect_kw(Keyword::In, "'in'");
        let iter = self.parse_expr();
        let body = self.parse_block();
        Stmt::For {
            target,
            iter,
            body,
            span: self.span_from(start),
        }
    }

    /// A for-loop / assignment target: a postfix expression, or a tuple of them.
    /// Does not consume binary operators (notably `in`).
    fn parse_target(&mut self) -> Expr {
        let start = self.peek_span().start;
        let first = self.parse_postfix();
        if self.is(&TokenKind::Comma) {
            let mut elems = vec![first];
            while self.eat(&TokenKind::Comma) {
                if self.is_kw(Keyword::In) || self.is(&TokenKind::Assign) {
                    break;
                }
                elems.push(self.parse_postfix());
            }
            Expr::Tuple(elems, self.span_from(start))
        } else {
            first
        }
    }

    fn parse_match(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Match, "'match'");
        let scrutinee = self.parse_expr();
        self.expect(&TokenKind::Colon, "':'");
        self.expect(&TokenKind::Newline, "newline");
        self.expect(&TokenKind::Indent, "indented block");
        let mut arms = Vec::new();
        while self.is_kw(Keyword::Case) {
            let astart = self.peek_span().start;
            self.bump();
            let pattern = self.parse_pattern();
            let guard = if self.eat_kw(Keyword::If) {
                Some(self.parse_expr())
            } else {
                None
            };
            let body = self.parse_block();
            arms.push(MatchArm {
                pattern,
                guard,
                body,
                span: self.span_from(astart),
            });
        }
        self.expect(&TokenKind::Dedent, "end of match block");
        Stmt::Match {
            scrutinee,
            arms,
            span: self.span_from(start),
        }
    }

    fn parse_try(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Try, "'try'");
        let body = self.parse_block();
        let mut excepts = Vec::new();
        while self.is_kw(Keyword::Except) {
            let estart = self.peek_span().start;
            self.bump();
            let ty = if self.is(&TokenKind::Colon) {
                None
            } else {
                Some(self.parse_type())
            };
            let binding = if self.eat_kw(Keyword::As) {
                Some(self.ident("binding name"))
            } else {
                None
            };
            let eb = self.parse_block();
            excepts.push(ExceptClause {
                ty,
                binding,
                body: eb,
                span: self.span_from(estart),
            });
        }
        let finally = if self.eat_kw(Keyword::Finally) {
            Some(self.parse_block())
        } else {
            None
        };
        Stmt::Try {
            body,
            excepts,
            finally,
            span: self.span_from(start),
        }
    }

    fn parse_with(&mut self) -> Stmt {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::With, "'with'");
        let mut items = Vec::new();
        loop {
            let istart = self.peek_span().start;
            let expr = self.parse_expr();
            let alias = if self.eat_kw(Keyword::As) {
                Some(self.ident("binding name"))
            } else {
                None
            };
            items.push(WithItem {
                expr,
                alias,
                span: self.span_from(istart),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let body = self.parse_block();
        Stmt::With {
            items,
            body,
            span: self.span_from(start),
        }
    }

    fn parse_simple_stmt(&mut self) -> Stmt {
        let start = self.peek_span().start;
        if self.eat_kw(Keyword::Return) {
            let value = if self.at_line_end() {
                None
            } else {
                // A bare comma-list is a tuple return (D-0049): `return a, b`
                // builds a tuple the caller destructures — there is no separate
                // multiple-return mechanism.
                let first = self.parse_expr();
                if self.is(&TokenKind::Comma) {
                    let mut elems = vec![first];
                    while self.eat(&TokenKind::Comma) {
                        if self.at_line_end() {
                            break; // trailing comma
                        }
                        elems.push(self.parse_expr());
                    }
                    Some(Expr::Tuple(elems, self.span_from(start)))
                } else {
                    Some(first)
                }
            };
            return Stmt::Return {
                value,
                span: self.span_from(start),
            };
        }
        if self.eat_kw(Keyword::Raise) {
            let value = if self.at_line_end() {
                None
            } else {
                Some(self.parse_expr())
            };
            return Stmt::Raise {
                value,
                span: self.span_from(start),
            };
        }
        if self.eat_kw(Keyword::Assert) {
            // `assert cond [, msg]` (D-0054). The condition's source span is
            // captured so power-assert can echo the exact expression text.
            let cond_start = self.peek_span().start;
            let cond = self.parse_expr();
            let cond_src = self.source_slice(Span::new(cond_start, self.prev_end()));
            let cond_src = cond_src.trim().to_string();
            let msg = if self.eat(&TokenKind::Comma) {
                Some(self.parse_expr())
            } else {
                None
            };
            return Stmt::Assert {
                cond,
                msg,
                cond_src,
                span: self.span_from(start),
            };
        }
        if self.eat_kw(Keyword::Break) {
            return Stmt::Break(self.span_from(start));
        }
        if self.eat_kw(Keyword::Continue) {
            return Stmt::Continue(self.span_from(start));
        }
        if self.eat_kw(Keyword::Spawn) {
            // `detached` is a contextual modifier (D-0044), not a reserved word:
            // valid only immediately after `spawn` and only when it introduces a
            // call (`spawn detached f(...)`). A bare `detached` identifier that is
            // itself the callee (`spawn detached()`) stays an ordinary call.
            let detached = matches!(self.peek(), TokenKind::Ident(n) if n == "detached")
                && !matches!(self.peek2(), TokenKind::LParen);
            if detached {
                self.bump();
            }
            let call = self.parse_expr();
            return Stmt::Spawn {
                call,
                detached,
                span: self.span_from(start),
            };
        }
        if let Some(decl) = self.try_declaration(start) {
            return decl;
        }
        // assignment / aug-assignment / substitution / expression statement
        let target = self.parse_expr();
        // Bare comma-list destructuring target (D-0049): `a, b = ...`,
        // `first, *rest = ...`. The first element was already parsed as an
        // expression; collect the rest as target elements (which may carry `*`).
        if self.is(&TokenKind::Comma) {
            let mut elems = vec![BindTarget::Elem(target)];
            while self.eat(&TokenKind::Comma) {
                if self.is(&TokenKind::Assign) || self.at_line_end() {
                    break; // trailing comma
                }
                elems.push(self.parse_target_elem());
            }
            let bt = BindTarget::Tuple(elems, self.span_from(start));
            if self.eat(&TokenKind::Assign) {
                let value = self.parse_rhs();
                return Stmt::AssignTuple {
                    target: bt,
                    value,
                    span: self.span_from(start),
                };
            }
            // Not an assignment: a bare tuple expression statement.
            return Stmt::Expr(bind_target_to_expr(bt));
        }
        if self.eat(&TokenKind::Assign) {
            // A parenthesized tuple on the left is a destructuring target
            // (`(x, y) = (y, x)`); anything else is an ordinary lvalue.
            if matches!(target, Expr::Tuple(..)) {
                let bt = expr_to_bind_target(target);
                let value = self.parse_rhs();
                return Stmt::AssignTuple {
                    target: bt,
                    value,
                    span: self.span_from(start),
                };
            }
            let value = self.parse_rhs();
            return Stmt::Assign {
                target,
                value,
                span: self.span_from(start),
            };
        }
        if let Some(op) = self.aug_assign_op() {
            self.bump();
            let value = self.parse_expr();
            return Stmt::AugAssign {
                target,
                op,
                value,
                span: self.span_from(start),
            };
        }
        if self.is(&TokenKind::SubstAssign) {
            self.bump();
            let sub = self.parse_subst_literal(target.clone());
            return Stmt::Assign {
                target,
                value: sub,
                span: self.span_from(start),
            };
        }
        Stmt::Expr(target)
    }

    fn at_line_end(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Newline | TokenKind::Semicolon | TokenKind::Dedent | TokenKind::Eof
        )
    }

    fn aug_assign_op(&self) -> Option<BinOp> {
        Some(match self.peek() {
            TokenKind::PlusEq => BinOp::Add,
            TokenKind::MinusEq => BinOp::Sub,
            TokenKind::StarEq => BinOp::Mul,
            TokenKind::SlashEq => BinOp::Div,
            TokenKind::SlashSlashEq => BinOp::FloorDiv,
            TokenKind::PercentEq => BinOp::Mod,
            TokenKind::StarStarEq => BinOp::Pow,
            TokenKind::AmpEq => BinOp::BitAnd,
            TokenKind::PipeEq => BinOp::BitOr,
            TokenKind::CaretEq => BinOp::BitXor,
            TokenKind::ShlEq => BinOp::Shl,
            TokenKind::ShrEq => BinOp::Shr,
            _ => return None,
        })
    }

    fn try_declaration(&mut self, start: u32) -> Option<Stmt> {
        let cp = (self.pos, self.errors.len());
        let is_const = self.eat_kw(Keyword::Const);
        // `auto bind_target = expr` — a single name stays a plain [`Stmt::Let`];
        // a tuple target (`auto (a, b) = t`, `auto first, *rest = xs`) becomes a
        // [`Stmt::LetTuple`] (D-0049).
        if self.eat_kw(Keyword::Auto) {
            let target = self.parse_bind_target();
            if self.eat(&TokenKind::Assign) {
                let value = self.parse_rhs();
                return Some(self.make_decl(is_const, None, target, value, start));
            }
            self.restore(cp);
            return None;
        }
        // `TYPE bind_target = expr`
        if self.looks_like_type_start() {
            let ty = self.parse_type();
            let target = self.parse_bind_target();
            if self.eat(&TokenKind::Assign) {
                let value = self.parse_rhs();
                return Some(self.make_decl(is_const, Some(ty), target, value, start));
            }
        }
        self.restore(cp);
        None
    }

    /// Build a declaration from a parsed bind target: a single `name` target is
    /// a plain [`Stmt::Let`]; any tuple/rest target is a [`Stmt::LetTuple`].
    fn make_decl(
        &self,
        is_const: bool,
        ty: Option<Type>,
        target: BindTarget,
        value: Expr,
        start: u32,
    ) -> Stmt {
        let span = self.span_from(start);
        if let BindTarget::Elem(Expr::Name(name, _)) = &target {
            return Stmt::Let {
                is_const,
                ty,
                name: name.clone(),
                value,
                span,
            };
        }
        Stmt::LetTuple {
            is_const,
            ty,
            target,
            value,
            span,
        }
    }

    /// The right-hand side of a declaration or assignment. A bare comma-list is a
    /// tuple value (D-0049), mirroring the bare comma-list tuple return — so
    /// `a, b = b, a` and `auto t = 1, 2` build tuples. A parenthesized tuple is
    /// handled by the ordinary expression parser.
    fn parse_rhs(&mut self) -> Expr {
        let start = self.peek_span().start;
        let first = self.parse_expr();
        if self.is(&TokenKind::Comma) {
            let mut elems = vec![first];
            while self.eat(&TokenKind::Comma) {
                if self.at_line_end() {
                    break; // trailing comma
                }
                elems.push(self.parse_expr());
            }
            Expr::Tuple(elems, self.span_from(start))
        } else {
            first
        }
    }

    /// A `bind_target` (D-0049): a single target, or a comma-list / parenthesized
    /// tuple of target elements. Used after `auto`/`TYPE` in a declaration.
    fn parse_bind_target(&mut self) -> BindTarget {
        let start = self.peek_span().start;
        let first = self.parse_target_elem();
        if self.is(&TokenKind::Comma) {
            let mut elems = vec![first];
            while self.eat(&TokenKind::Comma) {
                if self.is(&TokenKind::Assign) || self.at_line_end() {
                    break; // trailing comma
                }
                elems.push(self.parse_target_elem());
            }
            BindTarget::Tuple(elems, self.span_from(start))
        } else {
            first
        }
    }

    /// A `target_elem` (D-0049): `[*] target`, or a parenthesized nested tuple.
    fn parse_target_elem(&mut self) -> BindTarget {
        let start = self.peek_span().start;
        if self.eat(&TokenKind::Star) {
            let inner = self.parse_target_elem();
            return BindTarget::Rest(Box::new(inner), self.span_from(start));
        }
        if self.is(&TokenKind::LParen) {
            self.bump(); // '('
            let mut elems = Vec::new();
            let mut saw_comma = false;
            if !self.is(&TokenKind::RParen) {
                loop {
                    elems.push(self.parse_target_elem());
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                    saw_comma = true;
                    if self.is(&TokenKind::RParen) {
                        break; // trailing comma
                    }
                }
            }
            self.expect(&TokenKind::RParen, "')'");
            // A single parenthesized target with no comma is just grouping;
            // `(x,)` (one element with a trailing comma) is a 1-tuple.
            if elems.len() == 1 && !saw_comma && !matches!(elems[0], BindTarget::Rest(..)) {
                return elems.into_iter().next().unwrap();
            }
            return BindTarget::Tuple(elems, self.span_from(start));
        }
        BindTarget::Elem(self.parse_postfix())
    }

    fn restore(&mut self, cp: (usize, usize)) {
        self.pos = cp.0;
        self.errors.truncate(cp.1);
    }

    fn looks_like_type_start(&self) -> bool {
        match self.peek() {
            TokenKind::Keyword(k) => k.is_type(),
            TokenKind::Ident(_) => true,
            _ => false,
        }
    }

    // --- types ---

    fn parse_type(&mut self) -> Type {
        let start = self.peek_span().start;
        if self.eat(&TokenKind::LParen) {
            let mut elems = Vec::new();
            if !self.is(&TokenKind::RParen) {
                loop {
                    elems.push(self.parse_type());
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                    if self.is(&TokenKind::RParen) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RParen, "')'");
            return Type::Tuple {
                elems,
                span: self.span_from(start),
            };
        }
        let name = match self.peek() {
            TokenKind::Keyword(k) if k.is_type() => {
                let s = k.as_str().to_string();
                self.bump();
                s
            }
            TokenKind::Ident(n) => {
                let s = n.clone();
                self.bump();
                s
            }
            _ => {
                self.error_here("expected a type");
                "<error>".to_string()
            }
        };
        let mut args = Vec::new();
        if self.eat(&TokenKind::LBracket) {
            if !self.is(&TokenKind::RBracket) {
                loop {
                    let before = self.pos;
                    args.push(self.parse_type());
                    if self.pos == before {
                        self.bump(); // ensure progress
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RBracket, "']'");
        }
        Type::Named {
            name,
            args,
            span: self.span_from(start),
        }
    }

    // --- expressions ---

    fn parse_expr(&mut self) -> Expr {
        if self.is_kw(Keyword::Lambda) {
            return self.parse_lambda();
        }
        let start = self.peek_span().start;
        let value = self.parse_or_expr();
        if self.eat_kw(Keyword::If) {
            let cond = self.parse_or_expr();
            self.expect_kw(Keyword::Else, "'else'");
            let els = self.parse_expr();
            return Expr::IfElse {
                cond: Box::new(cond),
                then: Box::new(value),
                els: Box::new(els),
                span: self.span_from(start),
            };
        }
        value
    }

    fn parse_lambda(&mut self) -> Expr {
        let start = self.peek_span().start;
        self.expect_kw(Keyword::Lambda, "'lambda'");
        let mut params = Vec::new();
        if !self.is(&TokenKind::Colon) {
            loop {
                params.push(self.ident("lambda parameter"));
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::Colon, "':'");
        let body = self.parse_expr();
        Expr::Lambda {
            params,
            body: Box::new(body),
            span: self.span_from(start),
        }
    }

    fn parse_or_expr(&mut self) -> Expr {
        let start = self.peek_span().start;
        let mut lhs = self.parse_and_expr();
        while self.is_kw(Keyword::Or) {
            self.bump();
            let rhs = self.parse_and_expr();
            lhs = Expr::Binary {
                op: BinOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span: self.span_from(start),
            };
        }
        lhs
    }

    fn parse_and_expr(&mut self) -> Expr {
        let start = self.peek_span().start;
        let mut lhs = self.parse_not_expr();
        while self.is_kw(Keyword::And) {
            self.bump();
            let rhs = self.parse_not_expr();
            lhs = Expr::Binary {
                op: BinOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span: self.span_from(start),
            };
        }
        lhs
    }

    fn parse_not_expr(&mut self) -> Expr {
        if self.is_kw(Keyword::Not) {
            let start = self.peek_span().start;
            self.bump();
            let expr = self.parse_not_expr();
            return Expr::Unary {
                op: UnOp::Not,
                expr: Box::new(expr),
                span: self.span_from(start),
            };
        }
        self.parse_bin(0)
    }

    /// Precedence-climbing over comparison/bitwise/shift/arithmetic operators
    /// and the regex-bind operators. `min_bp` is the minimum left binding power.
    fn parse_bin(&mut self, min_bp: u8) -> Expr {
        let start = self.peek_span().start;
        let mut lhs = self.parse_operand();
        loop {
            // Regex bind (`=~` / `!~`) at comparison precedence.
            if matches!(self.peek(), TokenKind::RegexBind | TokenKind::RegexNotBind) {
                if 30 < min_bp {
                    break;
                }
                let negated = matches!(self.peek(), TokenKind::RegexNotBind);
                self.bump();
                lhs = self.parse_regex_rhs(lhs, negated, start);
                continue;
            }
            let Some((bp, op)) = self.bin_bp() else {
                break;
            };
            if bp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.parse_bin(bp + 1);
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span: self.span_from(start),
            };
        }
        lhs
    }

    fn bin_bp(&self) -> Option<(u8, BinOp)> {
        Some(match self.peek() {
            TokenKind::LArrow => (25, BinOp::Send),
            TokenKind::EqEq => (30, BinOp::Eq),
            TokenKind::Ne => (30, BinOp::Ne),
            TokenKind::Lt => (30, BinOp::Lt),
            TokenKind::Le => (30, BinOp::Le),
            TokenKind::Gt => (30, BinOp::Gt),
            TokenKind::Ge => (30, BinOp::Ge),
            TokenKind::Keyword(Keyword::In) => (30, BinOp::In),
            TokenKind::Pipe => (40, BinOp::BitOr),
            TokenKind::Caret => (50, BinOp::BitXor),
            TokenKind::Amp => (60, BinOp::BitAnd),
            TokenKind::Shl => (70, BinOp::Shl),
            TokenKind::Shr => (70, BinOp::Shr),
            TokenKind::Plus => (80, BinOp::Add),
            TokenKind::Minus => (80, BinOp::Sub),
            TokenKind::Star => (90, BinOp::Mul),
            TokenKind::Slash => (90, BinOp::Div),
            TokenKind::SlashSlash => (90, BinOp::FloorDiv),
            TokenKind::Percent => (90, BinOp::Mod),
            _ => return None,
        })
    }

    fn parse_regex_rhs(&mut self, subject: Expr, negated: bool, start: u32) -> Expr {
        match self.peek().clone() {
            TokenKind::RegexMatch { pattern, flags } => {
                self.bump();
                Expr::RegexMatch {
                    subject: Box::new(subject),
                    pattern,
                    flags,
                    negated,
                    span: self.span_from(start),
                }
            }
            TokenKind::RegexSubst {
                pattern,
                replacement,
                flags,
            } if !negated => {
                self.bump();
                Expr::RegexSubst {
                    subject: Box::new(subject),
                    pattern,
                    replacement,
                    flags,
                    span: self.span_from(start),
                }
            }
            _ => {
                self.error_here("expected a regex literal after '=~'/'!~'");
                subject
            }
        }
    }

    fn parse_subst_literal(&mut self, subject: Expr) -> Expr {
        let start = self.peek_span().start;
        match self.peek().clone() {
            TokenKind::RegexSubst {
                pattern,
                replacement,
                flags,
            } => {
                self.bump();
                Expr::RegexSubst {
                    subject: Box::new(subject),
                    pattern,
                    replacement,
                    flags,
                    span: self.span_from(start),
                }
            }
            _ => {
                self.error_here("expected a substitution literal after '~='");
                subject
            }
        }
    }

    /// Unary (`u_expr`): prefix `-`/`+`/`~`, prefix `<-` (receive), `yield`,
    /// else a `power`.
    fn parse_operand(&mut self) -> Expr {
        let start = self.peek_span().start;
        match self.peek() {
            TokenKind::Minus => {
                self.bump();
                Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(self.parse_operand()),
                    span: self.span_from(start),
                }
            }
            TokenKind::Plus => {
                self.bump();
                Expr::Unary {
                    op: UnOp::Pos,
                    expr: Box::new(self.parse_operand()),
                    span: self.span_from(start),
                }
            }
            TokenKind::Tilde => {
                self.bump();
                Expr::Unary {
                    op: UnOp::BitNot,
                    expr: Box::new(self.parse_operand()),
                    span: self.span_from(start),
                }
            }
            TokenKind::LArrow => {
                self.bump();
                Expr::ChanRecv {
                    chan: Box::new(self.parse_operand()),
                    span: self.span_from(start),
                }
            }
            TokenKind::Keyword(Keyword::Yield) => {
                self.bump();
                let value = if self.at_line_end() || self.is(&TokenKind::RParen) {
                    None
                } else {
                    Some(Box::new(self.parse_expr()))
                };
                Expr::Yield {
                    value,
                    span: self.span_from(start),
                }
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> Expr {
        let start = self.peek_span().start;
        let base = self.parse_postfix();
        if self.eat(&TokenKind::StarStar) {
            let exp = self.parse_operand(); // right-assoc, allows unary on RHS
            Expr::Binary {
                op: BinOp::Pow,
                lhs: Box::new(base),
                rhs: Box::new(exp),
                span: self.span_from(start),
            }
        } else {
            base
        }
    }

    fn parse_postfix(&mut self) -> Expr {
        let start = self.peek_span().start;
        let mut e = self.parse_atom();
        loop {
            match self.peek() {
                TokenKind::LParen => {
                    self.bump();
                    let args = self.parse_args();
                    e = Expr::Call {
                        callee: Box::new(e),
                        args,
                        span: self.span_from(start),
                    };
                }
                TokenKind::LBracket => {
                    e = self.parse_subscript(e, start);
                }
                TokenKind::Dot => {
                    self.bump();
                    let name = self.ident("attribute name");
                    e = Expr::Attr {
                        base: Box::new(e),
                        name,
                        span: self.span_from(start),
                    };
                }
                _ => break,
            }
        }
        e
    }

    fn parse_args(&mut self) -> Vec<Arg> {
        let mut args = Vec::new();
        if self.eat(&TokenKind::RParen) {
            return args;
        }
        loop {
            let astart = self.peek_span().start;
            // keyword argument: IDENT '=' (but not '==')
            let name = if matches!(self.peek(), TokenKind::Ident(_))
                && matches!(self.peek2(), TokenKind::Assign)
            {
                let (n, _) = self.ident_opt().unwrap();
                self.bump(); // '='
                Some(n)
            } else {
                None
            };
            let value = self.parse_expr();
            args.push(Arg {
                name,
                value,
                span: self.span_from(astart),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if self.is(&TokenKind::RParen) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "')'");
        args
    }

    fn parse_subscript(&mut self, base: Expr, start: u32) -> Expr {
        self.expect(&TokenKind::LBracket, "'['");
        let lo = if self.is(&TokenKind::Colon) || self.is(&TokenKind::RBracket) {
            None
        } else {
            Some(Box::new(self.parse_expr()))
        };
        if self.is(&TokenKind::Colon) {
            self.bump();
            let hi = if self.is(&TokenKind::Colon) || self.is(&TokenKind::RBracket) {
                None
            } else {
                Some(Box::new(self.parse_expr()))
            };
            let step = if self.eat(&TokenKind::Colon) {
                if self.is(&TokenKind::RBracket) {
                    None
                } else {
                    Some(Box::new(self.parse_expr()))
                }
            } else {
                None
            };
            self.expect(&TokenKind::RBracket, "']'");
            Expr::Slice {
                base: Box::new(base),
                lo,
                hi,
                step,
                span: self.span_from(start),
            }
        } else {
            self.expect(&TokenKind::RBracket, "']'");
            let index = lo.unwrap_or_else(|| {
                self.error_here("expected an index expression");
                Box::new(Expr::None(self.peek_span()))
            });
            Expr::Index {
                base: Box::new(base),
                index,
                span: self.span_from(start),
            }
        }
    }

    fn parse_atom(&mut self) -> Expr {
        let start = self.peek_span().start;
        let span = self.peek_span();
        match self.peek().clone() {
            TokenKind::Int(s) => {
                self.bump();
                Expr::Int(s, span)
            }
            TokenKind::Float(s) => {
                self.bump();
                Expr::Float(s, span)
            }
            TokenKind::Imaginary(s) => {
                self.bump();
                Expr::Imaginary(s, span)
            }
            TokenKind::Str { value, flags } => {
                self.bump();
                Expr::Str {
                    value,
                    raw: flags.raw,
                    bytes: flags.bytes,
                    fstring: flags.fstring,
                    span,
                }
            }
            TokenKind::Keyword(Keyword::True) => {
                self.bump();
                Expr::Bool(true, span)
            }
            TokenKind::Keyword(Keyword::False) => {
                self.bump();
                Expr::Bool(false, span)
            }
            TokenKind::Keyword(Keyword::None) => {
                self.bump();
                Expr::None(span)
            }
            TokenKind::Ident(name) => {
                self.bump();
                Expr::Name(name, span)
            }
            // Type/builtin keywords usable in value position (constructors,
            // conversions, built-in functions): list/dict/array/int64(...)/
            // range/channel/close/... `close` is reserved (SPEC §16) but is an
            // ordinary built-in function (design/0005-concurrency.md), so it
            // reads as a callable name here.
            TokenKind::Keyword(k)
                if k.is_type()
                    || k == Keyword::Range
                    || k == Keyword::Channel
                    || k == Keyword::Close =>
            {
                self.bump();
                Expr::Name(k.as_str().to_string(), span)
            }
            TokenKind::LParen => self.parse_paren_or_tuple(start),
            TokenKind::LBracket => self.parse_list(start),
            TokenKind::LBrace => self.parse_brace(start),
            _ => {
                self.error_here("expected an expression");
                self.bump();
                Expr::None(span)
            }
        }
    }

    fn parse_paren_or_tuple(&mut self, start: u32) -> Expr {
        self.expect(&TokenKind::LParen, "'('");
        if self.eat(&TokenKind::RParen) {
            return Expr::Tuple(Vec::new(), self.span_from(start));
        }
        let first = self.parse_expr();
        if self.is(&TokenKind::Comma) {
            let mut elems = vec![first];
            while self.eat(&TokenKind::Comma) {
                if self.is(&TokenKind::RParen) {
                    break;
                }
                elems.push(self.parse_expr());
            }
            self.expect(&TokenKind::RParen, "')'");
            Expr::Tuple(elems, self.span_from(start))
        } else {
            self.expect(&TokenKind::RParen, "')'");
            first
        }
    }

    fn parse_list(&mut self, start: u32) -> Expr {
        self.expect(&TokenKind::LBracket, "'['");
        if self.eat(&TokenKind::RBracket) {
            return Expr::List(Vec::new(), self.span_from(start));
        }
        let first = self.parse_expr();
        if self.is_kw(Keyword::For) {
            let (var, iter, cond) = self.parse_comp_clause();
            self.expect(&TokenKind::RBracket, "']'");
            return Expr::ListComp {
                elem: Box::new(first),
                var,
                iter: Box::new(iter),
                cond: cond.map(Box::new),
                span: self.span_from(start),
            };
        }
        let mut elems = vec![first];
        while self.eat(&TokenKind::Comma) {
            if self.is(&TokenKind::RBracket) {
                break;
            }
            elems.push(self.parse_expr());
        }
        self.expect(&TokenKind::RBracket, "']'");
        Expr::List(elems, self.span_from(start))
    }

    /// A brace literal: a **dict** when it has `key: value` pairs, a **set** when
    /// it has comma-separated elements and no colon. Empty `{}` stays a dict
    /// (the conventional default; an empty set has no literal form yet).
    fn parse_brace(&mut self, start: u32) -> Expr {
        self.expect(&TokenKind::LBrace, "'{'");
        if self.eat(&TokenKind::RBrace) {
            return Expr::Dict(Vec::new(), self.span_from(start));
        }
        let first = self.parse_expr();
        // A `:` after the first element means this is a dict; anything else
        // (`,`, `}`, or a `for`) means a set.
        if self.is(&TokenKind::Colon) {
            return self.parse_dict_rest(start, first);
        }
        self.parse_set_rest(start, first)
    }

    /// Finish a dict literal (or dict comprehension) whose first key is `first`.
    fn parse_dict_rest(&mut self, start: u32, first_key: Expr) -> Expr {
        self.expect(&TokenKind::Colon, "':'");
        let first_val = self.parse_expr();
        if self.is_kw(Keyword::For) {
            let (var, iter, cond) = self.parse_comp_clause();
            self.expect(&TokenKind::RBrace, "'}'");
            return Expr::DictComp {
                key: Box::new(first_key),
                value: Box::new(first_val),
                var,
                iter: Box::new(iter),
                cond: cond.map(Box::new),
                span: self.span_from(start),
            };
        }
        let mut items = vec![(first_key, first_val)];
        while self.eat(&TokenKind::Comma) {
            if self.is(&TokenKind::RBrace) {
                break;
            }
            let k = self.parse_expr();
            self.expect(&TokenKind::Colon, "':'");
            let v = self.parse_expr();
            items.push((k, v));
        }
        self.expect(&TokenKind::RBrace, "'}'");
        Expr::Dict(items, self.span_from(start))
    }

    /// Finish a set literal (or set comprehension) whose first element is `first`.
    fn parse_set_rest(&mut self, start: u32, first: Expr) -> Expr {
        if self.is_kw(Keyword::For) {
            let (var, iter, cond) = self.parse_comp_clause();
            self.expect(&TokenKind::RBrace, "'}'");
            return Expr::SetComp {
                elem: Box::new(first),
                var,
                iter: Box::new(iter),
                cond: cond.map(Box::new),
                span: self.span_from(start),
            };
        }
        let mut elems = vec![first];
        while self.eat(&TokenKind::Comma) {
            if self.is(&TokenKind::RBrace) {
                break;
            }
            elems.push(self.parse_expr());
        }
        self.expect(&TokenKind::RBrace, "'}'");
        Expr::Set(elems, self.span_from(start))
    }

    /// `for TARGET in ITER [if COND]` — the comprehension clause. The iterable
    /// and filter are parsed without a trailing ternary (so `if` is the filter).
    fn parse_comp_clause(&mut self) -> (String, Expr, Option<Expr>) {
        self.expect_kw(Keyword::For, "'for'");
        let var = self.ident("comprehension variable");
        self.expect_kw(Keyword::In, "'in'");
        let iter = self.parse_or_expr();
        let cond = if self.eat_kw(Keyword::If) {
            Some(self.parse_or_expr())
        } else {
            None
        };
        (var, iter, cond)
    }

    // --- patterns (match-case) ---

    fn parse_pattern(&mut self) -> Pattern {
        let first = self.parse_closed_pattern();
        if self.is(&TokenKind::Pipe) {
            let start = self.peek_span().start;
            let mut alts = vec![first];
            while self.eat(&TokenKind::Pipe) {
                alts.push(self.parse_closed_pattern());
            }
            Pattern::Or(alts, self.span_from(start))
        } else {
            first
        }
    }

    fn parse_closed_pattern(&mut self) -> Pattern {
        let start = self.peek_span().start;
        let span = self.peek_span();
        match self.peek().clone() {
            TokenKind::Ident(name) if name == "_" => {
                self.bump();
                Pattern::Wildcard(span)
            }
            TokenKind::Ident(name) => {
                self.bump();
                // A dotted/qualified name: `Color.RED`, `Shape.Circle`, ...
                // (D-0018/D-0046). Enum members/variants are matched qualified.
                let mut path = vec![name.clone()];
                while self.eat(&TokenKind::Dot) {
                    path.push(self.ident("qualified pattern name"));
                }
                // Struct / enum-variant pattern: `Path(pat_args)`. Arguments are
                // positional (`r`) or by name (`h=h`), developer's choice
                // (D-0040/D-0046).
                if self.is(&TokenKind::LParen) {
                    self.bump();
                    let mut args = Vec::new();
                    if !self.is(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_pat_arg());
                            if !self.eat(&TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    self.expect(&TokenKind::RParen, "')'");
                    Pattern::Struct {
                        path,
                        args: Some(args),
                        span: self.span_from(start),
                    }
                } else if path.len() == 1 {
                    // A bare name binds (capture, Python semantics, D-0040).
                    Pattern::Capture { name, span }
                } else {
                    // A qualified constant or payload-free variant with no
                    // parens: `Color.RED`, `Shape.Unit`.
                    Pattern::Struct {
                        path,
                        args: None,
                        span: self.span_from(start),
                    }
                }
            }
            TokenKind::Star => {
                self.bump();
                let name = self.ident_opt().map(|(n, _)| n);
                Pattern::Rest {
                    name,
                    span: self.span_from(start),
                }
            }
            TokenKind::LBracket => {
                self.bump();
                let mut pats = Vec::new();
                if !self.is(&TokenKind::RBracket) {
                    loop {
                        pats.push(self.parse_pattern());
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RBracket, "']'");
                Pattern::Sequence(pats, self.span_from(start))
            }
            TokenKind::LParen => {
                self.bump();
                let mut pats = Vec::new();
                if !self.is(&TokenKind::RParen) {
                    loop {
                        pats.push(self.parse_pattern());
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RParen, "')'");
                Pattern::Tuple(pats, self.span_from(start))
            }
            _ => {
                // Literal pattern (numbers, strings, True/False/None).
                let e = self.parse_operand();
                Pattern::Literal(e)
            }
        }
    }

    /// One argument of a struct/variant pattern: `IDENT "=" pattern` (by name)
    /// or `pattern` (positional, declaration order; D-0040/D-0046).
    fn parse_pat_arg(&mut self) -> PatArg {
        let start = self.peek_span().start;
        let named =
            matches!(self.peek(), TokenKind::Ident(_)) && matches!(self.peek2(), TokenKind::Assign);
        if named {
            let name = self.ident("field name");
            self.expect(&TokenKind::Assign, "'='");
            let pattern = self.parse_pattern();
            PatArg {
                name: Some(name),
                pattern,
                span: self.span_from(start),
            }
        } else {
            let pattern = self.parse_pattern();
            PatArg {
                name: None,
                pattern,
                span: self.span_from(start),
            }
        }
    }
}

/// Convert a parsed expression used as an assignment target into a
/// [`BindTarget`] (D-0049). A tuple expression destructures positionally; any
/// other expression is a single lvalue leaf. (`*rest` cannot appear in a parsed
/// expression, so parenthesized rest is only reachable via a declaration's
/// `parse_bind_target`.)
fn expr_to_bind_target(e: Expr) -> BindTarget {
    match e {
        Expr::Tuple(elems, span) => {
            BindTarget::Tuple(elems.into_iter().map(expr_to_bind_target).collect(), span)
        }
        other => BindTarget::Elem(other),
    }
}

/// Reconstruct a tuple expression from a bind target that turned out not to be
/// an assignment (a bare tuple expression statement, e.g. `a, b`).
fn bind_target_to_expr(bt: BindTarget) -> Expr {
    match bt {
        BindTarget::Elem(e) => e,
        BindTarget::Tuple(elems, span) => {
            Expr::Tuple(elems.into_iter().map(bind_target_to_expr).collect(), span)
        }
        // A `*rest` in expression position is not representable; fall back to the
        // inner expression (this only arises from malformed input).
        BindTarget::Rest(inner, _) => bind_target_to_expr(*inner),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Module {
        let (module, perrs, lerrs) = parse_str(src);
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        module
    }

    #[test]
    fn empty_module() {
        let m = parse_ok("");
        assert!(m.items.is_empty());
    }

    #[test]
    fn fib_fixture_parses() {
        let src = include_str!("../../../tests/golden/fib.zap");
        let m = parse_ok(src);
        assert_eq!(m.items.len(), 2);
        match &m.items[0] {
            Item::Function(f) => {
                assert_eq!(f.name, "fib");
                assert_eq!(f.params.len(), 1);
                assert_eq!(f.params[0].name, "n");
                assert!(matches!(&f.ret, Some(Type::Named { name, .. }) if name == "int64"));
            }
            _ => panic!("expected function"),
        }
    }

    #[test]
    fn assert_captures_condition_source() {
        let m = parse_ok("def main():\n    assert x + 1 == y, \"msg\"\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        match &f.body.stmts[0] {
            Stmt::Assert { cond_src, msg, .. } => {
                assert_eq!(cond_src, "x + 1 == y");
                assert!(msg.is_some(), "message should be parsed");
            }
            other => panic!("expected an assert, got {other:?}"),
        }
    }

    #[test]
    fn assert_without_message_parses() {
        let m = parse_ok("def main():\n    assert ok\n");
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        assert!(matches!(&f.body.stmts[0], Stmt::Assert { msg: None, .. }));
    }

    #[test]
    fn scope_block_and_spawn_detached_parse() {
        // `scope(...)` parses as a compound statement owning a `spawn`; the inner
        // `spawn` is not detached, while `spawn detached` sets the modifier and
        // `spawn detached()` (a call to a fn literally named `detached`) does not
        // (ticket #50, D-0044).
        let m = parse_ok(
            "def main():\n    scope(on_error=OnError.Wait):\n        spawn f()\n        spawn detached g()\n    spawn detached h()\n    spawn detached()\n",
        );
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        let stmts = &f.body.stmts;
        match &stmts[0] {
            Stmt::Scope { opts, body, .. } => {
                assert_eq!(opts.len(), 1);
                assert_eq!(opts[0].name.as_deref(), Some("on_error"));
                assert!(matches!(
                    &body.stmts[0],
                    Stmt::Spawn {
                        detached: false,
                        ..
                    }
                ));
                assert!(matches!(&body.stmts[1], Stmt::Spawn { detached: true, .. }));
            }
            other => panic!("expected a scope statement, got {other:?}"),
        }
        assert!(matches!(&stmts[1], Stmt::Spawn { detached: true, .. }));
        // `spawn detached()` — `detached` is the callee, not the modifier.
        assert!(matches!(
            &stmts[2],
            Stmt::Spawn {
                detached: false,
                ..
            }
        ));
    }

    #[test]
    fn loop_sum_fixture_parses() {
        let src = include_str!("../../../tests/golden/loop_sum.zap");
        let m = parse_ok(src);
        assert_eq!(m.items.len(), 1);
        match &m.items[0] {
            Item::Function(f) => {
                assert_eq!(f.name, "main");
                // auto total = 0 ; for ...: ... ; print(total)
                assert!(f.body.stmts.len() >= 3);
                assert!(matches!(f.body.stmts[0], Stmt::Let { .. }));
                assert!(matches!(f.body.stmts[1], Stmt::For { .. }));
            }
            _ => panic!("expected function"),
        }
    }

    #[test]
    fn declaration_forms() {
        let m = parse_ok("int64 count = 0\nauto name = \"zap\"\nconst f64 PI = 3.14\n");
        assert_eq!(m.items.len(), 3);
        assert!(matches!(
            &m.items[0],
            Item::Stmt(Stmt::Let {
                ty: Some(_),
                is_const: false,
                ..
            })
        ));
        assert!(matches!(
            &m.items[1],
            Item::Stmt(Stmt::Let { ty: None, .. })
        ));
        assert!(matches!(
            &m.items[2],
            Item::Stmt(Stmt::Let { is_const: true, .. })
        ));
    }

    #[test]
    fn assignment_vs_declaration() {
        // `x = 1` is an assignment, not a declaration.
        let m = parse_ok("x = 1\nx.y = 2\narr[0] = 3\n");
        assert!(matches!(&m.items[0], Item::Stmt(Stmt::Assign { .. })));
        assert!(matches!(&m.items[1], Item::Stmt(Stmt::Assign { .. })));
        assert!(matches!(&m.items[2], Item::Stmt(Stmt::Assign { .. })));
    }

    #[test]
    fn precedence_mul_over_add() {
        // 1 + 2 * 3  ->  1 + (2 * 3)
        let m = parse_ok("x = 1 + 2 * 3\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!("assign");
        };
        let Expr::Binary { op, rhs, .. } = value else {
            panic!("binary");
        };
        assert_eq!(*op, BinOp::Add);
        assert!(matches!(rhs.as_ref(), Expr::Binary { op: BinOp::Mul, .. }));
    }

    #[test]
    fn power_is_right_associative() {
        // 2 ** 3 ** 2 -> 2 ** (3 ** 2)
        let m = parse_ok("x = 2 ** 3 ** 2\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        let Expr::Binary { op, rhs, .. } = value else {
            panic!();
        };
        assert_eq!(*op, BinOp::Pow);
        assert!(matches!(rhs.as_ref(), Expr::Binary { op: BinOp::Pow, .. }));
    }

    #[test]
    fn aug_assign_and_call() {
        let m = parse_ok("total += f(x, y=2)\n");
        let Item::Stmt(Stmt::AugAssign { op, value, .. }) = &m.items[0] else {
            panic!("aug assign");
        };
        assert_eq!(*op, BinOp::Add);
        let Expr::Call { args, .. } = value else {
            panic!("call");
        };
        assert_eq!(args.len(), 2);
        assert_eq!(args[1].name.as_deref(), Some("y"));
    }

    #[test]
    fn comprehension_filter_not_ternary() {
        let m = parse_ok("xs = [x * x for x in range(10) if x > 1]\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(value, Expr::ListComp { cond: Some(_), .. }));
    }

    #[test]
    fn brace_literal_set_vs_dict() {
        // Comma-separated, no colon -> set.
        let m = parse_ok("s = {1, 2, 3}\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(value, Expr::Set(elems, _) if elems.len() == 3));
        // Key: value pairs -> dict.
        let m = parse_ok("d = {1: 2, 3: 4}\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(value, Expr::Dict(items, _) if items.len() == 2));
        // Empty braces stay a dict.
        let m = parse_ok("e = {}\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(value, Expr::Dict(items, _) if items.is_empty()));
        // A `for` with no colon -> set comprehension.
        let m = parse_ok("g = {x for x in range(3)}\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(value, Expr::SetComp { .. }));
    }

    #[test]
    fn ternary_expression() {
        let m = parse_ok("y = a if c else b\n");
        let Item::Stmt(Stmt::Assign { value, .. }) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(value, Expr::IfElse { .. }));
    }

    #[test]
    fn if_elif_else() {
        let src = "def f(int64 x):\n    if x > 0:\n        return 1\n    elif x == 0:\n        return 0\n    else:\n        return -1\n";
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        let Stmt::If {
            branches,
            else_block,
            ..
        } = &f.body.stmts[0]
        else {
            panic!("if");
        };
        assert_eq!(branches.len(), 2);
        assert!(else_block.is_some());
    }

    #[test]
    fn do_while_and_match() {
        let src = "def f():\n    do:\n        step()\n    while go\n    match x:\n        case 0:\n            a()\n        case _:\n            b()\n";
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(f.body.stmts[0], Stmt::DoWhile { .. }));
        let Stmt::Match { arms, .. } = &f.body.stmts[1] else {
            panic!("match");
        };
        assert_eq!(arms.len(), 2);
        assert!(matches!(arms[1].pattern, Pattern::Wildcard(_)));
    }

    #[test]
    fn struct_with_fields_and_method() {
        let src = "struct Point:\n    f64 x\n    f64 y\n    def norm(self) -> f64:\n        return self.x\n";
        let m = parse_ok(src);
        let Item::Struct(s) = &m.items[0] else {
            panic!("struct");
        };
        assert_eq!(s.name, "Point");
        assert_eq!(s.fields.len(), 2);
        assert_eq!(s.methods.len(), 1);
        assert_eq!(s.methods[0].params[0].name, "self");
        assert!(!s.is_value, "a plain struct is a reference type (D-0023)");
    }

    #[test]
    fn value_struct_modifier_parses() {
        // The contextual `value` modifier (D-0051) sets the flag; `value` stays a
        // usable identifier elsewhere.
        let m = parse_ok("value struct Vec2:\n    f64 x\n    f64 y\n");
        let Item::Struct(s) = &m.items[0] else {
            panic!("struct");
        };
        assert_eq!(s.name, "Vec2");
        assert!(s.is_value, "`value struct` should set is_value");

        // `value` not immediately before `struct` is an ordinary identifier.
        let m2 = parse_ok("def f():\n    auto value = 3\n    print(value)\n");
        assert!(matches!(&m2.items[0], Item::Function(_)));
    }

    #[test]
    fn value_struct_with_decorator_parses() {
        // Decorators precede the `value` modifier (`decorators [ "value" ]
        // "struct"`, grammar).
        let m = parse_ok("@derive(Eq)\nvalue struct Key:\n    int64 a\n");
        let Item::Struct(s) = &m.items[0] else {
            panic!("struct");
        };
        assert!(s.is_value);
        assert_eq!(s.decorators.len(), 1);
    }

    #[test]
    fn enum_and_import() {
        let m = parse_ok(
            "enum Color:\n    RED\n    GREEN = 10\n    BLUE\nimport python \"numpy\" as np\n",
        );
        let Item::Enum(e) = &m.items[0] else {
            panic!("enum");
        };
        assert_eq!(e.variants.len(), 3);
        assert!(e.variants[1].value.is_some());
        assert!(e.variants.iter().all(|v| v.fields.is_empty()));
        assert!(matches!(&m.items[1], Item::Import(Import::Python { .. })));
    }

    #[test]
    fn tier3_enum_variants_and_methods() {
        // Data-carrying variants with type-first named payload fields, a
        // payload-free variant, and a method in the body (D-0046).
        let src = "enum Shape:\n    Circle(f64 r)\n    Rect(f64 w, f64 h)\n    Unit\n\
                   \x20   def kind(self) -> int64:\n        return 0\n";
        let m = parse_ok(src);
        let Item::Enum(e) = &m.items[0] else {
            panic!("enum");
        };
        assert_eq!(e.variants.len(), 3);
        assert_eq!(e.variants[0].fields.len(), 1);
        assert_eq!(e.variants[0].fields[0].name, "r");
        assert_eq!(e.variants[1].fields.len(), 2);
        assert!(e.variants[2].fields.is_empty());
        assert_eq!(e.methods.len(), 1);
        assert_eq!(e.methods[0].name, "kind");
    }

    #[test]
    fn qualified_and_positional_patterns() {
        // `Color.RED` (dotted constant), `Shape.Circle(r)` (positional), and
        // `Shape.Rect(w=w, h=h)` (named) all parse (D-0040/D-0046).
        let src = "def f(Shape s):\n    match s:\n\
                   \x20       case Shape.Circle(r):\n            a()\n\
                   \x20       case Shape.Rect(w=w, h=h):\n            b()\n\
                   \x20       case Color.RED:\n            c()\n";
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        let Stmt::Match { arms, .. } = &f.body.stmts[0] else {
            panic!("match");
        };
        assert_eq!(arms.len(), 3);
        // Positional: one arg, no name.
        let Pattern::Struct { path, args, .. } = &arms[0].pattern else {
            panic!("variant pattern");
        };
        assert_eq!(path, &["Shape".to_string(), "Circle".to_string()]);
        let args = args.as_ref().expect("has parens");
        assert_eq!(args.len(), 1);
        assert!(args[0].name.is_none());
        // Named: both args carry field names.
        let Pattern::Struct { args, .. } = &arms[1].pattern else {
            panic!("variant pattern");
        };
        let args = args.as_ref().unwrap();
        assert_eq!(args[0].name.as_deref(), Some("w"));
        assert_eq!(args[1].name.as_deref(), Some("h"));
        // Bare qualified constant: no parens.
        let Pattern::Struct { path, args, .. } = &arms[2].pattern else {
            panic!("qualified constant pattern");
        };
        assert_eq!(path, &["Color".to_string(), "RED".to_string()]);
        assert!(args.is_none());
    }

    #[test]
    fn decorated_function() {
        let m = parse_ok("@cache\n@retry(times=3)\ndef fetch(str url) -> str:\n    return url\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.decorators.len(), 2);
        assert_eq!(f.decorators[0].name, "cache");
        assert_eq!(f.decorators[1].name, "retry");
        assert_eq!(f.decorators[1].args.len(), 1);
    }

    #[test]
    fn regex_match_and_subst_exprs() {
        let m = parse_ok("def f(str s):\n    if s =~ m/foo/i:\n        s ~= s/a/b/g\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        let Stmt::If { branches, .. } = &f.body.stmts[0] else {
            panic!("if");
        };
        assert!(matches!(
            branches[0].0,
            Expr::RegexMatch { negated: false, .. }
        ));
        assert!(matches!(branches[0].1.stmts[0], Stmt::Assign { .. }));
    }

    #[test]
    fn variadic_and_default_params() {
        let m = parse_ok("def g(int64 a, int64 b = 2, int64... rest) -> int64:\n    return a\n");
        let Item::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.params.len(), 3);
        assert!(f.params[1].default.is_some());
        assert!(f.params[2].variadic);
    }

    #[test]
    fn errors_are_recovered_not_panicked() {
        // Missing ')' — should produce errors, not panic.
        let (_m, perrs, _l) = parse_str("def f(:\n    return 1\n");
        assert!(!perrs.is_empty());
    }

    // --- tuples (ticket #54, D-0049) ---

    fn body(src: &str) -> Vec<Stmt> {
        let m = parse_ok(src);
        let Item::Function(f) = &m.items[0] else {
            panic!("expected a function");
        };
        f.body.stmts.clone()
    }

    #[test]
    fn single_name_decl_stays_plain_let() {
        // A non-destructuring `auto x = ...` is still a plain `Let`, not a tuple.
        let s = body("def f():\n    auto x = 1\n");
        assert!(matches!(&s[0], Stmt::Let { name, .. } if name == "x"));
    }

    #[test]
    fn parenthesized_tuple_decl_is_let_tuple() {
        let s = body("def f():\n    auto (a, b, c) = t\n");
        let Stmt::LetTuple { target, .. } = &s[0] else {
            panic!("expected LetTuple, got {:?}", s[0]);
        };
        let BindTarget::Tuple(elems, _) = target else {
            panic!("expected a tuple target");
        };
        assert_eq!(elems.len(), 3);
        assert!(matches!(&elems[0], BindTarget::Elem(Expr::Name(n, _)) if n == "a"));
    }

    #[test]
    fn bare_comma_decl_is_let_tuple() {
        // `auto a, b = ...` (no parentheses) also destructures.
        let s = body("def f():\n    auto a, b = t\n");
        assert!(matches!(&s[0], Stmt::LetTuple { .. }));
    }

    #[test]
    fn rest_target_parses() {
        let s = body("def f():\n    auto (first, *rest) = xs\n");
        let Stmt::LetTuple { target, .. } = &s[0] else {
            panic!("expected LetTuple");
        };
        let BindTarget::Tuple(elems, _) = target else {
            panic!("tuple target");
        };
        assert_eq!(elems.len(), 2);
        assert!(matches!(&elems[1], BindTarget::Rest(..)));
    }

    #[test]
    fn swap_assignment_is_assign_tuple() {
        let s = body("def f():\n    (x, y) = (y, x)\n");
        assert!(matches!(&s[0], Stmt::AssignTuple { .. }));
    }

    #[test]
    fn single_lvalue_assignment_stays_plain_assign() {
        let s = body("def f():\n    x = 1\n");
        assert!(matches!(&s[0], Stmt::Assign { .. }));
    }

    #[test]
    fn bare_comma_both_sides_swap() {
        // `a, b = b, a` — both the target and the value are bare comma-lists.
        let s = body("def f():\n    a, b = b, a\n");
        let Stmt::AssignTuple { value, .. } = &s[0] else {
            panic!("expected AssignTuple, got {:?}", s[0]);
        };
        assert!(matches!(value, Expr::Tuple(elems, _) if elems.len() == 2));
    }

    #[test]
    fn return_comma_list_is_a_tuple() {
        let s = body("def f():\n    return a, b\n");
        let Stmt::Return { value: Some(e), .. } = &s[0] else {
            panic!("expected return");
        };
        assert!(matches!(e, Expr::Tuple(elems, _) if elems.len() == 2));
    }

    #[test]
    fn one_tuple_needs_trailing_comma() {
        // `(x)` is grouping (a single `Let`); `(x,)` is a 1-tuple target.
        let s = body("def f():\n    auto (x) = t\n");
        assert!(matches!(&s[0], Stmt::Let { name, .. } if name == "x"));
        let s = body("def f():\n    auto (x,) = t\n");
        let Stmt::LetTuple { target, .. } = &s[0] else {
            panic!("expected LetTuple for a 1-tuple target");
        };
        assert!(matches!(target, BindTarget::Tuple(elems, _) if elems.len() == 1));
    }
}
