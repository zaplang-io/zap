//! Indentation-sensitive lexer (Phase 1).
//!
//! Turns Zap source into a flat [`Token`] stream, synthesizing the layout
//! tokens [`TokenKind::Newline`], [`TokenKind::Indent`] and
//! [`TokenKind::Dedent`] Python-style (D-0002/D-0041), and ending with
//! [`TokenKind::Eof`].
//!
//! Rules of note:
//! - Indentation is spaces only; a tab in indentation is an error (D-0041).
//! - Blank and comment-only lines produce no layout tokens.
//! - Inside `()`, `[]`, `{}` newlines are insignificant (implicit continuation);
//!   a trailing `\` also continues a line.
//! - `m`/`s` start a regex literal only when immediately followed by a `/` or
//!   `{` delimiter (D-0010); otherwise they are ordinary identifiers, so a bare
//!   `/` is always division.
//!
//! Numeric and string literals keep their raw text; escape processing,
//! f-string parsing and numeric typing happen in later phases.

use crate::span::Span;
use crate::token::{Keyword, StrFlags, Token, TokenKind};
use crate::trivia::{Comment, CommentPlacement};
use zapc_diag::ErrorCode;

/// A lexing error with its stable [`ErrorCode`] and the offending source span.
///
/// The `code` is the machine-parseable identifier (see `zapc_diag`); `message`
/// is human-facing prose that may change between releases.
#[derive(Clone, Debug, PartialEq)]
pub struct LexError {
    pub code: ErrorCode,
    pub message: String,
    pub span: Span,
}

/// Tokenize `src`, returning the token stream and any errors. The stream always
/// ends with [`TokenKind::Eof`] even when there are errors.
///
/// Comments are discarded here; use [`tokenize_with_trivia`] to also recover
/// them. This function's output is byte-identical whether or not trivia is
/// collected, so the parser and everything downstream are unaffected (#225).
pub fn tokenize(src: &str) -> (Vec<Token>, Vec<LexError>) {
    let (tokens, errors, _comments) = tokenize_with_trivia(src);
    (tokens, errors)
}

/// Tokenize `src` and additionally recover every `#` comment as trivia
/// (I#157/A1, #225). The returned `tokens`/`errors` are identical to what
/// [`tokenize`] produces; the extra `Vec<Comment>` is an additive side-channel
/// for tooling (`zap fmt`, `zap doc`) that must re-emit source faithfully.
pub fn tokenize_with_trivia(src: &str) -> (Vec<Token>, Vec<LexError>, Vec<Comment>) {
    let mut lexer = Lexer::new(src);
    lexer.run();
    (lexer.tokens, lexer.errors, lexer.comments)
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    tokens: Vec<Token>,
    errors: Vec<LexError>,
    comments: Vec<Comment>,
    indents: Vec<u32>,
    paren_depth: i32,
    line_has_tokens: bool,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Lexer<'a> {
        Lexer {
            src: src.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            errors: Vec::new(),
            comments: Vec::new(),
            indents: vec![0],
            paren_depth: 0,
            line_has_tokens: false,
        }
    }

    // --- cursor helpers ---

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<u8> {
        self.src.get(self.pos + n).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn at(&self, b: u8) -> bool {
        self.peek() == Some(b)
    }

    fn emit(&mut self, kind: TokenKind, start: usize) {
        self.tokens
            .push(Token::new(kind, Span::new(start as u32, self.pos as u32)));
    }

    fn error(&mut self, code: ErrorCode, message: impl Into<String>, start: usize) {
        self.errors.push(LexError {
            code,
            message: message.into(),
            span: Span::new(start as u32, self.pos as u32),
        });
    }

    // --- driver ---

    fn run(&mut self) {
        let mut at_line_start = true;
        loop {
            if at_line_start && self.paren_depth == 0 {
                let line_start = self.pos;
                let col = self.measure_indent(line_start);
                match self.peek() {
                    None => break,
                    Some(b'\n') | Some(b'#') => {
                        // Blank or comment-only line: no layout tokens. A `#`
                        // here is a standalone (own-line) comment — recorded as
                        // trivia but still producing no tokens (#225).
                        if self.at(b'#') {
                            self.consume_comment();
                        }
                        self.skip_to_eol();
                        if self.at(b'\n') {
                            self.bump();
                        }
                        continue;
                    }
                    _ => {}
                }
                self.apply_indent(col);
                at_line_start = false;
            }

            self.skip_inline_ws();
            if self.at(b'#') {
                // Inline/own-line comment mid-run (e.g. a trailing `# note`, or
                // a standalone comment inside brackets): recorded as trivia,
                // emits no token (#225).
                self.consume_comment();
                self.skip_to_eol();
            }

            match self.peek() {
                None => break,
                Some(b'\n') => {
                    self.bump();
                    if self.paren_depth == 0 {
                        self.emit(TokenKind::Newline, self.pos - 1);
                        self.line_has_tokens = false;
                        at_line_start = true;
                    }
                }
                Some(b'\\') if self.peek_at(1) == Some(b'\n') => {
                    self.bump();
                    self.bump();
                }
                Some(b'\\') if self.peek_at(1) == Some(b'\r') && self.peek_at(2) == Some(b'\n') => {
                    self.bump();
                    self.bump();
                    self.bump();
                }
                Some(_) => self.lex_token(),
            }
        }
        self.finalize();
    }

    fn finalize(&mut self) {
        if self.line_has_tokens {
            self.emit(TokenKind::Newline, self.pos);
            self.line_has_tokens = false;
        }
        while *self.indents.last().unwrap() > 0 {
            self.indents.pop();
            self.emit(TokenKind::Dedent, self.pos);
        }
        self.emit(TokenKind::Eof, self.pos);
    }

    /// Consume leading spaces, returning the indentation column. Errors on a tab.
    fn measure_indent(&mut self, line_start: usize) -> u32 {
        let mut col = 0u32;
        loop {
            match self.peek() {
                Some(b' ') => {
                    col += 1;
                    self.bump();
                }
                Some(b'\t') => {
                    self.error(
                        ErrorCode::ZAP1008,
                        "tabs are not allowed in indentation (use spaces)",
                        line_start,
                    );
                    self.bump(); // consume to make progress
                }
                Some(b'\r') => {
                    self.bump();
                }
                _ => break,
            }
        }
        col
    }

    fn apply_indent(&mut self, col: u32) {
        let top = *self.indents.last().unwrap();
        if col > top {
            self.indents.push(col);
            self.emit(TokenKind::Indent, self.pos);
        } else if col < top {
            while *self.indents.last().unwrap() > col {
                self.indents.pop();
                self.emit(TokenKind::Dedent, self.pos);
            }
            if *self.indents.last().unwrap() != col {
                self.error(ErrorCode::ZAP1005, "inconsistent dedent", self.pos);
                // Recover by treating this column as a new level.
                self.indents.push(col);
            }
        }
    }

    fn skip_inline_ws(&mut self) {
        while matches!(self.peek(), Some(b' ') | Some(b'\t') | Some(b'\r')) {
            self.bump();
        }
    }

    fn skip_to_eol(&mut self) {
        while !matches!(self.peek(), None | Some(b'\n')) {
            self.bump();
        }
    }

    /// Record the `#` comment starting at the cursor as trivia (#225). Does not
    /// advance the cursor — the caller's `skip_to_eol` consumes it. The stored
    /// lexeme keeps the leading `#` and drops trailing whitespace/CR.
    fn consume_comment(&mut self) {
        let start = self.pos;
        let mut end = start;
        while !matches!(self.src.get(end).copied(), None | Some(b'\n')) {
            end += 1;
        }
        while end > start && matches!(self.src[end - 1], b' ' | b'\t' | b'\r') {
            end -= 1;
        }
        let text = String::from_utf8_lossy(&self.src[start..end]).into_owned();
        let placement = self.comment_placement(start);
        self.comments.push(Comment {
            span: Span::new(start as u32, end as u32),
            text,
            placement,
        });
    }

    /// Classify a comment by scanning back to the previous newline: own-line if
    /// only whitespace precedes the `#`, otherwise trailing (end-of-line). Driven
    /// off the source so it is correct even for comments inside `()`/`[]`/`{}`,
    /// where the lexer's line state is not reset.
    fn comment_placement(&self, hash_pos: usize) -> CommentPlacement {
        let mut i = hash_pos;
        while i > 0 {
            match self.src[i - 1] {
                b'\n' => break,
                b' ' | b'\t' | b'\r' => i -= 1,
                _ => return CommentPlacement::EndOfLine,
            }
        }
        CommentPlacement::OwnLine
    }

    // --- token dispatch ---

    fn lex_token(&mut self) {
        let start = self.pos;
        let b = self.peek().unwrap();
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.lex_ident_or_prefixed(start),
            b'0'..=b'9' => self.lex_number(start),
            b'.' if matches!(self.peek_at(1), Some(b'0'..=b'9')) => self.lex_number(start),
            b'\'' | b'"' => self.lex_string(start, StrFlags::default()),
            _ => self.lex_operator(start),
        }
    }

    fn is_ident_continue(b: u8) -> bool {
        matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
    }

    /// Lex an identifier/keyword — but first check for a string prefix
    /// (`r"..."`, `b'...'`, `f"..."`, `rb`/`rf`) or a regex literal (`m/.../`,
    /// `s/.../.../`).
    fn lex_ident_or_prefixed(&mut self, start: usize) {
        // String prefix? up to two of r/b/f then a quote.
        if let Some(flags) = self.try_string_prefix() {
            self.lex_string(start, flags);
            return;
        }

        // Read the identifier.
        while let Some(b) = self.peek() {
            if Self::is_ident_continue(b) {
                self.bump();
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");

        // Contextual regex literal: `m`/`s` immediately followed by `/` or `{`.
        if (text == "m" || text == "s") && matches!(self.peek(), Some(b'/') | Some(b'{')) {
            let is_subst = text == "s";
            self.lex_regex(start, is_subst);
            return;
        }

        self.line_has_tokens = true;
        match Keyword::from_ident(text) {
            Some(kw) => self.emit(TokenKind::Keyword(kw), start),
            None => self.emit(TokenKind::Ident(text.to_string()), start),
        }
    }

    /// If the cursor is at a string prefix (`[rbf]{1,2}` then a quote), consume
    /// the prefix and return its flags; otherwise leave the cursor unchanged.
    fn try_string_prefix(&mut self) -> Option<StrFlags> {
        let mut flags = StrFlags::default();
        let mut n = 0usize;
        while n < 2 {
            match self.peek_at(n) {
                Some(b'r') | Some(b'R') => flags.raw = true,
                Some(b'b') | Some(b'B') => flags.bytes = true,
                Some(b'f') | Some(b'F') => flags.fstring = true,
                _ => break,
            }
            n += 1;
        }
        if n > 0 && matches!(self.peek_at(n), Some(b'\'') | Some(b'"')) {
            for _ in 0..n {
                self.bump();
            }
            Some(flags)
        } else {
            None
        }
    }

    fn lex_string(&mut self, start: usize, flags: StrFlags) {
        self.line_has_tokens = true;
        let quote = self.bump().unwrap(); // ' or "
        let triple = self.peek() == Some(quote) && self.peek_at(1) == Some(quote);
        if triple {
            self.bump();
            self.bump();
        }
        let body_start = self.pos;
        loop {
            match self.peek() {
                None => {
                    self.error(ErrorCode::ZAP1001, "unterminated string literal", start);
                    break;
                }
                Some(b'\\') => {
                    // Keep escapes verbatim; they never terminate the string.
                    self.bump();
                    if self.peek().is_some() {
                        self.bump();
                    }
                }
                Some(c) if c == quote => {
                    if triple {
                        if self.peek_at(1) == Some(quote) && self.peek_at(2) == Some(quote) {
                            let value = self.slice_string(body_start, self.pos);
                            self.bump();
                            self.bump();
                            self.bump();
                            self.emit(TokenKind::Str { value, flags }, start);
                            return;
                        }
                        self.bump();
                    } else {
                        let value = self.slice_string(body_start, self.pos);
                        self.bump();
                        self.emit(TokenKind::Str { value, flags }, start);
                        return;
                    }
                }
                Some(b'\n') if !triple => {
                    self.error(ErrorCode::ZAP1001, "unterminated string literal", start);
                    break;
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
        // Error recovery: emit what we have.
        let value = self.slice_string(body_start, self.pos);
        self.emit(TokenKind::Str { value, flags }, start);
    }

    fn slice_string(&self, from: usize, to: usize) -> String {
        String::from_utf8_lossy(&self.src[from..to]).into_owned()
    }

    fn lex_regex(&mut self, start: usize, is_subst: bool) {
        self.line_has_tokens = true;
        let open = self.bump().unwrap(); // '/' or '{'
        let close = if open == b'{' { b'}' } else { open };
        let pattern = self.read_regex_part(close, start);
        if !self.at(close) {
            self.error(ErrorCode::ZAP1002, "unterminated regex literal", start);
            self.emit_regex(start, is_subst, pattern, String::new());
            return;
        }
        self.bump(); // closing delimiter of pattern

        if is_subst {
            // Second part. For bracket delimiters expect a fresh `{...}`.
            if open == b'{' {
                if !self.at(b'{') {
                    self.error(
                        ErrorCode::ZAP1004,
                        "expected replacement delimiter in s{...}{...}",
                        start,
                    );
                    self.emit_regex(start, true, pattern, String::new());
                    return;
                }
                self.bump();
            }
            let replacement = self.read_regex_part(close, start);
            if !self.at(close) {
                self.error(
                    ErrorCode::ZAP1003,
                    "unterminated substitution literal",
                    start,
                );
                self.emit_regex(start, true, pattern, replacement);
                return;
            }
            self.bump();
            let flags = self.read_regex_flags();
            self.tokens.push(Token::new(
                TokenKind::RegexSubst {
                    pattern,
                    replacement,
                    flags,
                },
                Span::new(start as u32, self.pos as u32),
            ));
        } else {
            let flags = self.read_regex_flags();
            self.tokens.push(Token::new(
                TokenKind::RegexMatch { pattern, flags },
                Span::new(start as u32, self.pos as u32),
            ));
        }
    }

    fn emit_regex(&mut self, start: usize, is_subst: bool, pattern: String, replacement: String) {
        let kind = if is_subst {
            TokenKind::RegexSubst {
                pattern,
                replacement,
                flags: String::new(),
            }
        } else {
            TokenKind::RegexMatch {
                pattern,
                flags: String::new(),
            }
        };
        self.tokens
            .push(Token::new(kind, Span::new(start as u32, self.pos as u32)));
    }

    fn read_regex_part(&mut self, close: u8, start: usize) -> String {
        let from = self.pos;
        loop {
            match self.peek() {
                None => break,
                Some(b'\n') => {
                    self.error(ErrorCode::ZAP1002, "unterminated regex literal", start);
                    break;
                }
                Some(b'\\') => {
                    self.bump();
                    if self.peek().is_some() {
                        self.bump();
                    }
                }
                Some(c) if c == close => break,
                Some(_) => {
                    self.bump();
                }
            }
        }
        self.slice_string(from, self.pos)
    }

    fn read_regex_flags(&mut self) -> String {
        let from = self.pos;
        while matches!(self.peek(), Some(b'a'..=b'z') | Some(b'A'..=b'Z')) {
            self.bump();
        }
        self.slice_string(from, self.pos)
    }

    fn lex_number(&mut self, start: usize) {
        self.line_has_tokens = true;
        // Prefixed integers: 0x / 0o / 0b.
        if self.at(b'0') {
            if let Some(radix) = self.peek_at(1) {
                let is_prefix = matches!(radix, b'x' | b'X' | b'o' | b'O' | b'b' | b'B');
                if is_prefix {
                    self.bump();
                    self.bump();
                    while matches!(
                        self.peek(),
                        Some(b'0'..=b'9') | Some(b'a'..=b'f') | Some(b'A'..=b'F') | Some(b'_')
                    ) {
                        self.bump();
                    }
                    let text = self.slice_string(start, self.pos);
                    self.emit(TokenKind::Int(text), start);
                    return;
                }
            }
        }

        let mut is_float = false;

        // Integer / mantissa digits.
        while matches!(self.peek(), Some(b'0'..=b'9') | Some(b'_')) {
            self.bump();
        }
        // Fraction.
        if self.at(b'.') && !matches!(self.peek_at(1), Some(b'.')) {
            is_float = true;
            self.bump();
            while matches!(self.peek(), Some(b'0'..=b'9') | Some(b'_')) {
                self.bump();
            }
        }
        // Exponent.
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            let sign = matches!(self.peek_at(1), Some(b'+') | Some(b'-'));
            let after = if sign { 2 } else { 1 };
            if matches!(self.peek_at(after), Some(b'0'..=b'9')) {
                is_float = true;
                self.bump();
                if sign {
                    self.bump();
                }
                while matches!(self.peek(), Some(b'0'..=b'9') | Some(b'_')) {
                    self.bump();
                }
            }
        }
        // Imaginary suffix.
        if matches!(self.peek(), Some(b'j') | Some(b'J')) {
            self.bump();
            let text = self.slice_string(start, self.pos);
            self.emit(TokenKind::Imaginary(text), start);
            return;
        }

        let text = self.slice_string(start, self.pos);
        if is_float {
            self.emit(TokenKind::Float(text), start);
        } else {
            self.emit(TokenKind::Int(text), start);
        }
    }

    fn lex_operator(&mut self, start: usize) {
        let b = self.bump().unwrap();
        self.line_has_tokens = true;
        let kind = match b {
            b'(' => {
                self.paren_depth += 1;
                TokenKind::LParen
            }
            b')' => {
                self.paren_depth = (self.paren_depth - 1).max(0);
                TokenKind::RParen
            }
            b'[' => {
                self.paren_depth += 1;
                TokenKind::LBracket
            }
            b']' => {
                self.paren_depth = (self.paren_depth - 1).max(0);
                TokenKind::RBracket
            }
            b'{' => {
                self.paren_depth += 1;
                TokenKind::LBrace
            }
            b'}' => {
                self.paren_depth = (self.paren_depth - 1).max(0);
                TokenKind::RBrace
            }
            b',' => TokenKind::Comma,
            b';' => TokenKind::Semicolon,
            b':' => TokenKind::Colon,
            b'@' => TokenKind::At,
            b'~' => {
                if self.eat(b'=') {
                    TokenKind::SubstAssign
                } else {
                    TokenKind::Tilde
                }
            }
            b'+' => {
                if self.eat(b'=') {
                    TokenKind::PlusEq
                } else {
                    TokenKind::Plus
                }
            }
            b'%' => {
                if self.eat(b'=') {
                    TokenKind::PercentEq
                } else {
                    TokenKind::Percent
                }
            }
            b'&' => {
                if self.eat(b'=') {
                    TokenKind::AmpEq
                } else {
                    TokenKind::Amp
                }
            }
            b'|' => {
                if self.eat(b'=') {
                    TokenKind::PipeEq
                } else {
                    TokenKind::Pipe
                }
            }
            b'^' => {
                if self.eat(b'=') {
                    TokenKind::CaretEq
                } else {
                    TokenKind::Caret
                }
            }
            b'-' => {
                if self.eat(b'>') {
                    TokenKind::Arrow
                } else if self.eat(b'=') {
                    TokenKind::MinusEq
                } else {
                    TokenKind::Minus
                }
            }
            b'*' => {
                if self.eat(b'*') {
                    if self.eat(b'=') {
                        TokenKind::StarStarEq
                    } else {
                        TokenKind::StarStar
                    }
                } else if self.eat(b'=') {
                    TokenKind::StarEq
                } else {
                    TokenKind::Star
                }
            }
            b'/' => {
                if self.eat(b'/') {
                    if self.eat(b'=') {
                        TokenKind::SlashSlashEq
                    } else {
                        TokenKind::SlashSlash
                    }
                } else if self.eat(b'=') {
                    TokenKind::SlashEq
                } else {
                    TokenKind::Slash
                }
            }
            b'=' => {
                if self.eat(b'=') {
                    TokenKind::EqEq
                } else if self.eat(b'~') {
                    TokenKind::RegexBind
                } else {
                    TokenKind::Assign
                }
            }
            b'!' => {
                if self.eat(b'=') {
                    TokenKind::Ne
                } else if self.eat(b'~') {
                    TokenKind::RegexNotBind
                } else {
                    self.error(
                        ErrorCode::ZAP1007,
                        "unexpected `!` (did you mean `!=` or `not`?)",
                        start,
                    );
                    return;
                }
            }
            b'<' => {
                if self.eat(b'-') {
                    TokenKind::LArrow
                } else if self.eat(b'<') {
                    if self.eat(b'=') {
                        TokenKind::ShlEq
                    } else {
                        TokenKind::Shl
                    }
                } else if self.eat(b'=') {
                    TokenKind::Le
                } else {
                    TokenKind::Lt
                }
            }
            b'>' => {
                if self.eat(b'>') {
                    if self.eat(b'=') {
                        TokenKind::ShrEq
                    } else {
                        TokenKind::Shr
                    }
                } else if self.eat(b'=') {
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            b'.' => {
                if self.at(b'.') && self.peek_at(1) == Some(b'.') {
                    self.bump();
                    self.bump();
                    TokenKind::Ellipsis
                } else {
                    TokenKind::Dot
                }
            }
            other => {
                self.error(
                    ErrorCode::ZAP1006,
                    format!("unexpected character {:?}", other as char),
                    start,
                );
                return;
            }
        };
        self.emit(kind, start);
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.at(b) {
            self.bump();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokenize, asserting no errors, returning only the kinds.
    fn kinds(src: &str) -> Vec<TokenKind> {
        let (toks, errs) = tokenize(src);
        assert!(errs.is_empty(), "unexpected lex errors: {errs:?}");
        toks.into_iter().map(|t| t.kind).collect()
    }

    use Keyword as K;
    use TokenKind as T;

    #[test]
    fn empty_source() {
        assert_eq!(kinds(""), vec![T::Eof]);
    }

    #[test]
    fn simple_assignment() {
        // `x = 1` -> Ident Assign Int Newline Eof
        assert_eq!(
            kinds("x = 1\n"),
            vec![
                T::Ident("x".into()),
                T::Assign,
                T::Int("1".into()),
                T::Newline,
                T::Eof,
            ]
        );
    }

    #[test]
    fn trailing_newline_synthesized() {
        // No trailing '\n' in the source, but one NEWLINE is still emitted.
        assert_eq!(
            kinds("x = 1"),
            vec![
                T::Ident("x".into()),
                T::Assign,
                T::Int("1".into()),
                T::Newline,
                T::Eof,
            ]
        );
    }

    #[test]
    fn keywords_and_types() {
        assert_eq!(
            kinds("def auto int64 True None"),
            vec![
                T::Keyword(K::Def),
                T::Keyword(K::Auto),
                T::Keyword(K::Int64),
                T::Keyword(K::True),
                T::Keyword(K::None),
                T::Newline,
                T::Eof,
            ]
        );
    }

    #[test]
    fn numbers() {
        assert_eq!(
            kinds("0xFF 0b1010 0o17 1_000 3.14 .5 1. 1e10 2.5e-3 2j"),
            vec![
                T::Int("0xFF".into()),
                T::Int("0b1010".into()),
                T::Int("0o17".into()),
                T::Int("1_000".into()),
                T::Float("3.14".into()),
                T::Float(".5".into()),
                T::Float("1.".into()),
                T::Float("1e10".into()),
                T::Float("2.5e-3".into()),
                T::Imaginary("2j".into()),
                T::Newline,
                T::Eof,
            ]
        );
    }

    #[test]
    fn operators_longest_match() {
        assert_eq!(
            kinds("a // b ** c <<= d -> e <- f"),
            vec![
                T::Ident("a".into()),
                T::SlashSlash,
                T::Ident("b".into()),
                T::StarStar,
                T::Ident("c".into()),
                T::ShlEq,
                T::Ident("d".into()),
                T::Arrow,
                T::Ident("e".into()),
                T::LArrow,
                T::Ident("f".into()),
                T::Newline,
                T::Eof,
            ]
        );
    }

    #[test]
    fn strings_and_prefixes() {
        let ks = kinds(r#"x = "hi" + r"\d+" + f"{x}" + b'bytes'"#);
        assert!(ks.contains(&T::Str {
            value: "hi".into(),
            flags: StrFlags::default(),
        }));
        assert!(ks.contains(&T::Str {
            value: "\\d+".into(),
            flags: StrFlags {
                raw: true,
                bytes: false,
                fstring: false,
            },
        }));
        assert!(ks.contains(&T::Str {
            value: "{x}".into(),
            flags: StrFlags {
                raw: false,
                bytes: false,
                fstring: true,
            },
        }));
    }

    #[test]
    fn triple_quoted_string_spans_lines() {
        let ks = kinds("x = \"\"\"a\nb\"\"\"\n");
        assert!(ks.contains(&T::Str {
            value: "a\nb".into(),
            flags: StrFlags::default(),
        }));
    }

    #[test]
    fn regex_match_and_subst() {
        let ks = kinds("if s =~ m/\\bfoo\\d+/i:\n    name ~= s/a/b/g\n");
        assert!(ks.contains(&T::RegexMatch {
            pattern: "\\bfoo\\d+".into(),
            flags: "i".into(),
        }));
        assert!(ks.contains(&T::RegexSubst {
            pattern: "a".into(),
            replacement: "b".into(),
            flags: "g".into(),
        }));
    }

    #[test]
    fn m_is_identifier_without_delimiter() {
        // `m + 1` keeps `m` an identifier; `m / 1` is division (space before /).
        assert_eq!(
            kinds("m / 1"),
            vec![
                T::Ident("m".into()),
                T::Slash,
                T::Int("1".into()),
                T::Newline,
                T::Eof,
            ]
        );
    }

    #[test]
    fn indentation_blocks() {
        let src = "def f():\n    if x:\n        return 1\n    return 2\n";
        assert_eq!(
            kinds(src),
            vec![
                T::Keyword(K::Def),
                T::Ident("f".into()),
                T::LParen,
                T::RParen,
                T::Colon,
                T::Newline,
                T::Indent,
                T::Keyword(K::If),
                T::Ident("x".into()),
                T::Colon,
                T::Newline,
                T::Indent,
                T::Keyword(K::Return),
                T::Int("1".into()),
                T::Newline,
                T::Dedent,
                T::Keyword(K::Return),
                T::Int("2".into()),
                T::Newline,
                T::Dedent,
                T::Eof,
            ]
        );
    }

    #[test]
    fn blank_and_comment_lines_skipped() {
        let src = "x = 1\n\n   # a comment\ny = 2\n";
        assert_eq!(
            kinds(src),
            vec![
                T::Ident("x".into()),
                T::Assign,
                T::Int("1".into()),
                T::Newline,
                T::Ident("y".into()),
                T::Assign,
                T::Int("2".into()),
                T::Newline,
                T::Eof,
            ]
        );
    }

    #[test]
    fn implicit_continuation_inside_brackets() {
        // The newline inside (...) is insignificant: no NEWLINE token there.
        let src = "f(a,\n  b)\n";
        assert_eq!(
            kinds(src),
            vec![
                T::Ident("f".into()),
                T::LParen,
                T::Ident("a".into()),
                T::Comma,
                T::Ident("b".into()),
                T::RParen,
                T::Newline,
                T::Eof,
            ]
        );
    }

    #[test]
    fn backslash_line_continuation() {
        let src = "x = 1 + \\\n    2\n";
        assert_eq!(
            kinds(src),
            vec![
                T::Ident("x".into()),
                T::Assign,
                T::Int("1".into()),
                T::Plus,
                T::Int("2".into()),
                T::Newline,
                T::Eof,
            ]
        );
    }

    #[test]
    fn tab_indentation_is_an_error() {
        let (_toks, errs) = tokenize("def f():\n\treturn 1\n");
        assert!(!errs.is_empty(), "tab in indentation should error");
    }

    // --- comment trivia (#225) ---

    use crate::trivia::CommentPlacement as CP;

    /// The plain `tokenize` path and the trivia path produce byte-identical
    /// token streams — the core behavior-preservation guarantee of #225.
    #[test]
    fn trivia_path_tokens_are_identical() {
        let srcs = [
            "x = 1\n",
            "# header\nx = 1  # trailing\n\n# between\ny = 2\n",
            "def f():\n    # body comment\n    return 1  # ret\n",
            "xs = [\n    1,  # one\n    # dangling\n    2,\n]\n",
            "def fib(int64 n) -> int64:\n    if n < 2:\n        return n\n    return fib(n - 1)\n",
        ];
        for src in srcs {
            let (plain, plain_errs) = tokenize(src);
            let (with, with_errs, _comments) = tokenize_with_trivia(src);
            assert_eq!(plain, with, "token streams differ for {src:?}");
            assert_eq!(plain_errs, with_errs, "errors differ for {src:?}");
        }
    }

    #[test]
    fn own_line_and_trailing_comments() {
        let src = "# header\nx = 1  # trailing\n";
        let (_t, errs, comments) = tokenize_with_trivia(src);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].text, "# header");
        assert_eq!(comments[0].placement, CP::OwnLine);
        assert_eq!(comments[1].text, "# trailing");
        assert_eq!(comments[1].placement, CP::EndOfLine);
        // Spans point at the real source text (trailing whitespace trimmed).
        let c0 = comments[0].span;
        assert_eq!(&src[c0.start as usize..c0.end as usize], "# header");
    }

    #[test]
    fn comment_only_and_blank_lines() {
        let src = "x = 1\n\n   # indented standalone\ny = 2\n";
        let (_t, _e, comments) = tokenize_with_trivia(src);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].text, "# indented standalone");
        assert_eq!(comments[0].placement, CP::OwnLine);
    }

    #[test]
    fn dangling_comment_inside_brackets() {
        // A comment on its own line inside brackets is still own-line, even
        // though the lexer keeps `line_has_tokens` set across the continuation.
        let src = "xs = [\n    1,\n    # dangling\n    2,  # two\n]\n";
        let (_t, _e, comments) = tokenize_with_trivia(src);
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].text, "# dangling");
        assert_eq!(comments[0].placement, CP::OwnLine);
        assert_eq!(comments[1].text, "# two");
        assert_eq!(comments[1].placement, CP::EndOfLine);
    }

    #[test]
    fn comment_whitespace_is_trimmed_but_inner_kept() {
        let src = "x = 1  #   spaced out   \n";
        let (_t, _e, comments) = tokenize_with_trivia(src);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].text, "#   spaced out");
    }

    #[test]
    fn trailing_eof_comment_without_newline() {
        let src = "x = 1\n# last";
        let (_t, _e, comments) = tokenize_with_trivia(src);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].text, "# last");
        assert_eq!(comments[0].placement, CP::OwnLine);
    }

    #[test]
    fn no_comments_yields_empty_trivia() {
        let (_t, _e, comments) = tokenize_with_trivia("def f():\n    return 1\n");
        assert!(comments.is_empty());
    }

    #[test]
    fn fib_fixture_lexes_cleanly() {
        let src = "def fib(int64 n) -> int64:\n    if n < 2:\n        return n\n    return fib(n - 1) + fib(n - 2)\n\ndef main():\n    print(fib(10))\n";
        let (toks, errs) = tokenize(src);
        assert!(errs.is_empty(), "fib should lex without errors: {errs:?}");
        assert_eq!(toks.last().map(|t| &t.kind), Some(&TokenKind::Eof));
        // Balanced INDENT/DEDENT.
        let indents = toks.iter().filter(|t| t.kind == TokenKind::Indent).count();
        let dedents = toks.iter().filter(|t| t.kind == TokenKind::Dedent).count();
        assert_eq!(indents, dedents, "INDENT/DEDENT must balance");
    }
}
