//! f-string field splitting and format-spec parsing (D-0026).
//!
//! An f-string literal (`f"...{expr[!conv][:spec]}..."`) is kept **opaque** in
//! the AST as an [`Expr::Str`] with `fstring == true`, so `zap fmt` / `zap doc`
//! re-emit it verbatim. The expansion into a concatenation of `str` pieces
//! (design/0007-strings.md, "f-string lowering") happens later, in the type
//! checker (which type-checks each field expression) and the lowerer (which
//! builds the string). Both share this module so they agree on the split.
//!
//! The replacement-field grammar follows Python's:
//!   - `{{` / `}}` are literal braces;
//!   - a field is `{ expr [ !conv ] [ : spec ] }` where `expr` is a full Zap
//!     expression (sub-parsed via [`crate::parser::parse_expr_fragment`]);
//!   - `conv` is one of `r`/`s`/`a` (only `s` is supported in v1; `r`/`a` need
//!     the Display protocol, #274);
//!   - `spec` is the format mini-language parsed by [`parse_format_spec`].
//!
//! Field expressions and specs carry best-effort source spans: callers pass the
//! source offset of the literal's first content byte as `value_base`, and this
//! module offsets field spans from there. A caller that cannot determine the
//! exact offset (the prefix/quote width is not on the AST node) passes an
//! approximation, so field diagnostics land inside the f-string literal.

use crate::ast::Expr;
use crate::parser::{parse_expr_fragment, ParseError};
use crate::span::Span;
use zapc_diag::ErrorCode;

/// One piece of a split f-string.
#[derive(Clone, Debug)]
pub enum FStringPart {
    /// A literal text run (with `{{`/`}}` already reduced to single braces).
    /// Escapes (`\n`, …) are **not** processed here — the lowerer unescapes it
    /// exactly as it would a plain string literal (respecting the `raw` flag).
    Text(String),
    /// A `{ expr [!conv] [:spec] }` replacement field.
    Field {
        expr: Expr,
        /// The `!r`/`!s`/`!a` conversion, if any.
        conv: Option<char>,
        /// The parsed format spec, if a `:spec` was present and well-formed.
        spec: Option<FormatSpec>,
        /// Whether a `:spec` was written at all (even if it failed to parse).
        had_spec: bool,
        /// Span of the whole `{...}` field, for diagnostics.
        field_span: Span,
    },
}

/// Text alignment within a field of a given width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// `<` — left.
    Left,
    /// `>` — right.
    Right,
    /// `^` — centered.
    Center,
    /// `=` — pad between sign and digits (numeric only).
    SignAware,
}

/// A parsed format spec: `[[fill]align][sign][#][0][width][.precision][type]`.
#[derive(Clone, Debug, PartialEq)]
pub struct FormatSpec {
    pub fill: char,
    pub align: Option<Align>,
    /// `'+'`, `'-'` (default) or `' '`.
    pub sign: char,
    /// `#` — alternate form (`0x`/`0o`/`0b` prefix for int types).
    pub alt: bool,
    /// `0` — zero-pad to width (sign-aware).
    pub zero: bool,
    pub width: Option<u32>,
    pub precision: Option<u32>,
    /// Presentation type: `b o x X d` (int), `e E f F g G` (float), `s` (str).
    pub ty: Option<char>,
}

impl Default for FormatSpec {
    fn default() -> FormatSpec {
        FormatSpec {
            fill: ' ',
            align: None,
            sign: '-',
            alt: false,
            zero: false,
            width: None,
            precision: None,
            ty: None,
        }
    }
}

impl FormatSpec {
    /// Presentation types that select integer rendering.
    pub fn is_int_type(c: char) -> bool {
        matches!(c, 'b' | 'o' | 'x' | 'X' | 'd')
    }
    /// Presentation types that select float rendering.
    pub fn is_float_type(c: char) -> bool {
        matches!(c, 'e' | 'E' | 'f' | 'F' | 'g' | 'G')
    }
}

/// Parse a format-spec string (the part after `:` in a field, or `format`'s
/// second argument). Returns a human-facing message on a malformed spec.
pub fn parse_format_spec(spec: &str) -> Result<FormatSpec, String> {
    let chars: Vec<char> = spec.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut out = FormatSpec::default();

    let is_align = |c: char| matches!(c, '<' | '>' | '^' | '=');
    let align_of = |c: char| match c {
        '<' => Align::Left,
        '>' => Align::Right,
        '^' => Align::Center,
        _ => Align::SignAware,
    };

    // [[fill]align]
    if n >= 2 && is_align(chars[1]) {
        out.fill = chars[0];
        out.align = Some(align_of(chars[1]));
        i = 2;
    } else if n >= 1 && is_align(chars[0]) {
        out.align = Some(align_of(chars[0]));
        i = 1;
    }

    // [sign]
    if i < n && matches!(chars[i], '+' | '-' | ' ') {
        out.sign = chars[i];
        i += 1;
    }

    // [#]
    if i < n && chars[i] == '#' {
        out.alt = true;
        i += 1;
    }

    // [0]  (zero-pad; implies sign-aware alignment unless one was given)
    if i < n && chars[i] == '0' {
        out.zero = true;
        if out.align.is_none() {
            out.align = Some(Align::SignAware);
            out.fill = '0';
        }
        i += 1;
    }

    // [width]
    let wstart = i;
    while i < n && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i > wstart {
        let w: String = chars[wstart..i].iter().collect();
        out.width = Some(w.parse().map_err(|_| "width is too large".to_string())?);
    }

    // [.precision]
    if i < n && chars[i] == '.' {
        i += 1;
        let pstart = i;
        while i < n && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i == pstart {
            return Err("a '.' in a format spec must be followed by a precision".to_string());
        }
        let p: String = chars[pstart..i].iter().collect();
        out.precision = Some(
            p.parse()
                .map_err(|_| "precision is too large".to_string())?,
        );
    }

    // [type]
    if i < n {
        let t = chars[i];
        if FormatSpec::is_int_type(t) || FormatSpec::is_float_type(t) || t == 's' {
            out.ty = Some(t);
            i += 1;
        } else {
            return Err(format!("unknown format type '{t}'"));
        }
    }

    if i != n {
        return Err(format!("unexpected characters in format spec: '{}'", spec));
    }
    Ok(out)
}

fn perr(code: ErrorCode, message: impl Into<String>, span: Span) -> ParseError {
    ParseError {
        code,
        message: message.into(),
        span,
    }
}

/// Split an f-string's raw inner text into literal runs and replacement fields.
///
/// `value` is the literal's inner text (exactly as it appears between the
/// quotes, escapes unprocessed). `value_base` is the source offset of
/// `value[0]`, used to place field spans. Errors are collected rather than
/// aborting, so a best-effort split is always returned.
pub fn split_fstring(value: &str, value_base: u32) -> (Vec<FStringPart>, Vec<ParseError>) {
    let b = value.as_bytes();
    let n = b.len();
    let mut parts = Vec::new();
    let mut errors = Vec::new();
    let mut lit = String::new();
    let mut i = 0usize;

    let span_at = |lo: usize, hi: usize| {
        Span::new(
            value_base.saturating_add(lo as u32),
            value_base.saturating_add(hi as u32),
        )
    };

    while i < n {
        let c = b[i];
        if c == b'{' {
            if i + 1 < n && b[i + 1] == b'{' {
                lit.push('{');
                i += 2;
                continue;
            }
            if !lit.is_empty() {
                parts.push(FStringPart::Text(std::mem::take(&mut lit)));
            }
            let field_start = i;
            let expr_start = i + 1;
            let mut j = expr_start;
            let mut depth: i32 = 0;
            let mut expr_end: Option<usize> = None;
            let mut conv: Option<char> = None;
            let mut spec_range: Option<(usize, usize)> = None;
            let mut field_end: Option<usize> = None;

            while j < n {
                let cj = b[j];
                match cj {
                    b'\'' | b'"' => {
                        // Skip an embedded string literal so its `}`/`:`/`!`
                        // don't terminate the field.
                        let q = cj;
                        let triple = j + 2 < n && b[j + 1] == q && b[j + 2] == q;
                        j += if triple { 3 } else { 1 };
                        while j < n {
                            if b[j] == b'\\' {
                                j += 2;
                                continue;
                            }
                            if triple {
                                if j + 2 < n && b[j] == q && b[j + 1] == q && b[j + 2] == q {
                                    j += 3;
                                    break;
                                }
                                j += 1;
                            } else {
                                if b[j] == q {
                                    j += 1;
                                    break;
                                }
                                j += 1;
                            }
                        }
                    }
                    b'(' | b'[' | b'{' => {
                        depth += 1;
                        j += 1;
                    }
                    b')' | b']' | b'}' if depth > 0 => {
                        depth -= 1;
                        j += 1;
                    }
                    b'}' => {
                        if expr_end.is_none() {
                            expr_end = Some(j);
                        }
                        field_end = Some(j + 1);
                        break;
                    }
                    b'!' if depth == 0
                        && j + 1 < n
                        && matches!(b[j + 1], b'r' | b's' | b'a')
                        && (j + 2 >= n || b[j + 2] == b'}' || b[j + 2] == b':') =>
                    {
                        if expr_end.is_none() {
                            expr_end = Some(j);
                        }
                        conv = Some(b[j + 1] as char);
                        j += 2;
                    }
                    b':' if depth == 0 => {
                        if expr_end.is_none() {
                            expr_end = Some(j);
                        }
                        let sstart = j + 1;
                        let mut k = sstart;
                        while k < n && b[k] != b'}' {
                            k += 1;
                        }
                        spec_range = Some((sstart, k));
                        if k < n {
                            field_end = Some(k + 1);
                        }
                        break;
                    }
                    _ => {
                        j += 1;
                    }
                }
            }

            let fe = match field_end {
                Some(x) => x,
                None => {
                    errors.push(perr(
                        ErrorCode::ZAP2017,
                        "unterminated f-string field: missing '}'",
                        span_at(field_start, n),
                    ));
                    // Consume the rest so we don't loop.
                    i = n;
                    continue;
                }
            };
            let ee = expr_end.unwrap_or(fe.saturating_sub(1)).min(n);
            let expr_src = &value[expr_start..ee];
            let fspan = span_at(field_start, fe);

            if expr_src.trim().is_empty() {
                errors.push(perr(
                    ErrorCode::ZAP2017,
                    "empty f-string field: '{}' has no expression",
                    fspan,
                ));
                i = fe;
                continue;
            }

            let (expr, perrs) =
                parse_expr_fragment(expr_src, value_base.saturating_add(expr_start as u32));
            errors.extend(perrs);

            let (spec, had_spec) = match spec_range {
                Some((s, e)) => {
                    let raw = &value[s..e];
                    match parse_format_spec(raw) {
                        Ok(fs) => (Some(fs), true),
                        Err(msg) => {
                            errors.push(perr(
                                ErrorCode::ZAP4026,
                                format!("invalid format spec '{raw}': {msg}"),
                                span_at(s, e),
                            ));
                            (None, true)
                        }
                    }
                }
                None => (None, false),
            };

            parts.push(FStringPart::Field {
                expr,
                conv,
                spec,
                had_spec,
                field_span: fspan,
            });
            i = fe;
            continue;
        } else if c == b'}' {
            if i + 1 < n && b[i + 1] == b'}' {
                lit.push('}');
                i += 2;
                continue;
            }
            errors.push(perr(
                ErrorCode::ZAP2017,
                "single '}' in f-string (use '}}' for a literal brace)",
                span_at(i, i + 1),
            ));
            i += 1;
        } else {
            let ch = value[i..].chars().next().unwrap();
            lit.push(ch);
            i += ch.len_utf8();
        }
    }

    if !lit.is_empty() {
        parts.push(FStringPart::Text(lit));
    }
    (parts, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(value: &str) -> Vec<FStringPart> {
        let (parts, errs) = split_fstring(value, 0);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        parts
    }

    #[test]
    fn plain_text_is_one_part() {
        let p = fields("hello");
        assert_eq!(p.len(), 1);
        assert!(matches!(&p[0], FStringPart::Text(t) if t == "hello"));
    }

    #[test]
    fn escaped_braces() {
        let p = fields("{{x}}");
        assert_eq!(p.len(), 1);
        assert!(matches!(&p[0], FStringPart::Text(t) if t == "{x}"));
    }

    #[test]
    fn simple_field() {
        let p = fields("n={n}");
        assert_eq!(p.len(), 2);
        assert!(matches!(&p[0], FStringPart::Text(t) if t == "n="));
        assert!(matches!(&p[1], FStringPart::Field { spec: None, .. }));
    }

    #[test]
    fn field_with_spec() {
        let p = fields("{x:02x}");
        assert_eq!(p.len(), 1);
        match &p[0] {
            FStringPart::Field { spec: Some(fs), .. } => {
                assert_eq!(fs.width, Some(2));
                assert!(fs.zero);
                assert_eq!(fs.ty, Some('x'));
            }
            _ => panic!("expected a field with a spec"),
        }
    }

    #[test]
    fn nested_braces_and_strings() {
        // A dict literal and a string with a brace inside the field.
        let p = fields("{d[\"a}b\"]}");
        assert_eq!(p.len(), 1);
        assert!(matches!(&p[0], FStringPart::Field { .. }));
    }

    #[test]
    fn single_close_brace_errors() {
        let (_p, errs) = split_fstring("a}b", 0);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].code, ErrorCode::ZAP2017);
    }

    #[test]
    fn spec_grammar() {
        assert_eq!(parse_format_spec("02x").unwrap().ty, Some('x'));
        assert_eq!(parse_format_spec(">10").unwrap().align, Some(Align::Right));
        assert_eq!(parse_format_spec("*^8").unwrap().fill, '*');
        assert_eq!(parse_format_spec(".2f").unwrap().precision, Some(2));
        assert!(parse_format_spec("02q").is_err());
    }
}
