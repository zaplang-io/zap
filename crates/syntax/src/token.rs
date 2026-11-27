//! Lexical tokens.
//!
//! Numeric and string literals keep their raw lexeme text; interpretation
//! (e.g. `bigint` vs `int64`, escape processing, f-string parsing) happens in
//! later phases. Layout tokens (`Newline`, `Indent`, `Dedent`) are produced by
//! the indentation-sensitive lexer (Phase 1).

use crate::span::Span;

/// A token: its kind plus the source span it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Token {
        Token { kind, span }
    }
}

/// Prefix flags on a string literal (`r"..."`, `b"..."`, `f"..."`, `rb`, `rf`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StrFlags {
    pub raw: bool,
    pub bytes: bool,
    pub fstring: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    // --- Literals (raw lexeme text retained) ---
    /// Integer literal: `42`, `0x2A`, `0o52`, `0b101010`, `1_000`.
    Int(String),
    /// Float literal: `3.14`, `.5`, `1.`, `1e10`.
    Float(String),
    /// Imaginary literal: `2j`, `3.5j`.
    Imaginary(String),
    /// String literal: inner text plus prefix flags.
    Str {
        value: String,
        flags: StrFlags,
    },

    // --- Regex literals (D-0010) ---
    /// `m/PATTERN/FLAGS`
    RegexMatch {
        pattern: String,
        flags: String,
    },
    /// `s/PATTERN/REPLACEMENT/FLAGS`
    RegexSubst {
        pattern: String,
        replacement: String,
        flags: String,
    },

    // --- Identifiers & keywords ---
    Ident(String),
    Keyword(Keyword),

    // --- Operators & punctuation ---
    Plus,         // +
    Minus,        // -
    Star,         // *
    Slash,        // /
    SlashSlash,   // //
    Percent,      // %
    StarStar,     // **
    Amp,          // &
    Pipe,         // |
    Caret,        // ^
    Tilde,        // ~
    Shl,          // <<
    Shr,          // >>
    Assign,       // =
    EqEq,         // ==
    Ne,           // !=
    Lt,           // <
    Le,           // <=
    Gt,           // >
    Ge,           // >=
    Arrow,        // ->
    Colon,        // :
    ColonEq,      // :=   (walrus assignment-expression, #600)
    Comma,        // ,
    Dot,          // .
    Ellipsis,     // ...  (variadic, D-0038)
    At,           // @    (decorator, D-0036; infix matrix-multiply, #509)
    AtEq,         // @=   (in-place matrix-multiply, #509)
    Semicolon,    // ;
    LArrow,       // <-   (channel receive/send, D-0031)
    RegexBind,    // =~
    RegexNotBind, // !~
    SubstAssign,  // ~=  (in-place substitution, D-0010)

    // --- Augmented assignment (D-0026) ---
    PlusEq,       // +=
    MinusEq,      // -=
    StarEq,       // *=
    SlashEq,      // /=
    SlashSlashEq, // //=
    PercentEq,    // %=
    StarStarEq,   // **=
    AmpEq,        // &=
    PipeEq,       // |=
    CaretEq,      // ^=
    ShlEq,        // <<=
    ShrEq,        // >>=

    // --- Delimiters ---
    LParen,   // (
    RParen,   // )
    LBracket, // [
    RBracket, // ]
    LBrace,   // {
    RBrace,   // }

    // --- Layout (lexer-synthesized) ---
    Newline,
    Indent,
    Dedent,
    Eof,
}

/// Reserved words. Includes value keywords (`True`/`False`/`None`, D-0041) and
/// the built-in type keywords (which cannot be used as identifiers, D-0003).
///
/// Note: `m` and `s` are *contextual* (they start a regex literal only when
/// immediately followed by a delimiter, D-0010) and so are lexed as identifiers
/// elsewhere — they are intentionally absent from this table.
///
/// Likewise the *soft (contextual) keywords* are absent: each is a keyword only
/// in its single statement/clause slot and an ordinary identifier everywhere
/// else, so the lexer emits them as `Ident` and the parser recognizes them in
/// their slot. Tier 1 (#1405/#1411, epic #1413): `match`, `case`, `select`,
/// `scope`. Tier 2 (#1412): `spawn`, `when`, `do`, `unsafe`, `python`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
    // Declarations / control flow
    Def,
    Return,
    If,
    Elif,
    Else,
    While,
    For,
    In,
    Range,
    // NOTE: the soft (contextual) keywords are absent from this table — lexed as
    // `Ident` (like `m`/`s`/`default`) and recognized by the parser only in
    // their slot. Tier 1 (#1405/#1411, epic #1413): `match`/`case` (the
    // `match EXPR:` statement + its `case` arms), `select`/`scope`. Tier 2
    // (#1412): `do` (the `do:`…`while` statement), `when` (the `when target.*:`
    // conditional-compilation statement/item), `spawn`, `unsafe`, `python`.
    /// `comptime` — the compile-time evaluation surface (E2/E3, epic #1296):
    /// `comptime for f in …:` (a compile-time-unrolled loop) and `comptime <expr>`
    /// (force compile-time evaluation). Locked decision 4 of epic #1249.
    Comptime,
    Break,
    Continue,
    Auto,
    Const,
    Raise,
    Assert,
    Try,
    Except,
    Finally,
    With,
    As,
    Yield,
    Lambda,
    Import,
    // Concurrency
    Channel,
    // NOTE (continued): `spawn` (statement-leading `spawn <call>`), `select`,
    // `scope`, `unsafe`, `python` (the `import python "mod"` interop slot) are
    // soft keywords too — see the note above; none has a `Keyword` variant.
    Close,
    // Type/decl keywords
    Struct,
    Interface,
    Enum,
    Annotation,
    Decorator,
    // Value keywords
    True,
    False,
    None,
    // Boolean operators
    And,
    Or,
    Not,
    /// `is` — identity comparison (`a is b`, `x is None`), and the first token
    /// of the two-token `is not` operator (#611, D18). A reserved keyword, as
    /// in Python.
    Is,
    // Built-in type names
    Int8,
    Int16,
    Int32,
    Int64,
    Float32,
    Float64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    // Unsigned integer type names (#552, D-0019 amendment). Reintroduced for C
    // FFI (#539) and array dtypes: C ABIs are saturated with `unsigned`/`size_t`.
    // Each has a `uintN` and a short `uN` spelling, mirroring `int32`/`i32`;
    // `usize` is the single pointer-width name (no short form, no `size_t`
    // alias — D-0019 locks "no type aliases").
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    U8,
    U16,
    U32,
    U64,
    Usize,
    Bool,
    Char,
    Str,
    Bytes,
    Bigint,
    Complex,
    List,
    Dict,
    Set,
    Heap,
    Array,
}

impl Keyword {
    /// Map an identifier to its keyword, if it is reserved.
    pub fn from_ident(s: &str) -> Option<Keyword> {
        use Keyword::*;
        Some(match s {
            "def" => Def,
            "return" => Return,
            "if" => If,
            "elif" => Elif,
            "else" => Else,
            "while" => While,
            "for" => For,
            "in" => In,
            "range" => Range,
            // soft keywords lexed as idents: `do`/`when` (#1412), `match`/`case`
            // (#1411).
            "comptime" => Comptime,
            "break" => Break,
            "continue" => Continue,
            "auto" => Auto,
            "const" => Const,
            "raise" => Raise,
            "assert" => Assert,
            "try" => Try,
            "except" => Except,
            "finally" => Finally,
            "with" => With,
            "as" => As,
            "yield" => Yield,
            "lambda" => Lambda,
            "import" => Import,
            "channel" => Channel,
            // soft keywords lexed as idents: `spawn`/`unsafe`/`python` (#1412),
            // `select`/`scope` (#1411/#1405).
            "close" => Close,
            "struct" => Struct,
            "interface" => Interface,
            "enum" => Enum,
            "annotation" => Annotation,
            "decorator" => Decorator,
            "True" => True,
            "False" => False,
            "None" => None,
            "and" => And,
            "or" => Or,
            "not" => Not,
            "is" => Is,
            "int8" => Int8,
            "int16" => Int16,
            "int32" => Int32,
            "int64" => Int64,
            "float32" => Float32,
            "float64" => Float64,
            "i8" => I8,
            "i16" => I16,
            "i32" => I32,
            "i64" => I64,
            "f32" => F32,
            "f64" => F64,
            "uint8" => Uint8,
            "uint16" => Uint16,
            "uint32" => Uint32,
            "uint64" => Uint64,
            "u8" => U8,
            "u16" => U16,
            "u32" => U32,
            "u64" => U64,
            "usize" => Usize,
            "bool" => Bool,
            "char" => Char,
            "str" => Str,
            "bytes" => Bytes,
            "bigint" => Bigint,
            "complex" => Complex,
            "list" => List,
            "dict" => Dict,
            "set" => Set,
            "heap" => Heap,
            "array" => Array,
            _ => return Option::None,
        })
    }

    /// Is this keyword a built-in **nominal type name** (`int64`, `str`, `set`,
    /// `list`, `dict`, …)? These are hard keywords in type position, but a `.name`
    /// member position is unambiguous, so they are also accepted there as member
    /// names (#1237): without this a method literally called `set` — e.g.
    /// `ContextVar.set`, matching Python `contextvars` — could not be parsed.
    pub fn is_nominal_type(self) -> bool {
        use Keyword::*;
        matches!(
            self,
            Int8 | Int16
                | Int32
                | Int64
                | Float32
                | Float64
                | I8
                | I16
                | I32
                | I64
                | F32
                | F64
                | Uint8
                | Uint16
                | Uint32
                | Uint64
                | U8
                | U16
                | U32
                | U64
                | Usize
                | Bool
                | Char
                | Str
                | Bytes
                | Bigint
                | Complex
                | List
                | Dict
                | Set
                | Heap
                | Array
                | Range
                | Channel
        )
    }

    /// The source spelling of this keyword.
    pub fn as_str(self) -> &'static str {
        use Keyword::*;
        match self {
            Def => "def",
            Return => "return",
            If => "if",
            Elif => "elif",
            Else => "else",
            While => "while",
            For => "for",
            In => "in",
            Range => "range",
            Comptime => "comptime",
            Break => "break",
            Continue => "continue",
            Auto => "auto",
            Const => "const",
            Raise => "raise",
            Assert => "assert",
            Try => "try",
            Except => "except",
            Finally => "finally",
            With => "with",
            As => "as",
            Yield => "yield",
            Lambda => "lambda",
            Import => "import",
            Channel => "channel",
            Close => "close",
            Struct => "struct",
            Interface => "interface",
            Enum => "enum",
            Annotation => "annotation",
            Decorator => "decorator",
            True => "True",
            False => "False",
            None => "None",
            And => "and",
            Or => "or",
            Not => "not",
            Is => "is",
            Int8 => "int8",
            Int16 => "int16",
            Int32 => "int32",
            Int64 => "int64",
            Float32 => "float32",
            Float64 => "float64",
            I8 => "i8",
            I16 => "i16",
            I32 => "i32",
            I64 => "i64",
            F32 => "f32",
            F64 => "f64",
            Uint8 => "uint8",
            Uint16 => "uint16",
            Uint32 => "uint32",
            Uint64 => "uint64",
            U8 => "u8",
            U16 => "u16",
            U32 => "u32",
            U64 => "u64",
            Usize => "usize",
            Bool => "bool",
            Char => "char",
            Str => "str",
            Bytes => "bytes",
            Bigint => "bigint",
            Complex => "complex",
            List => "list",
            Dict => "dict",
            Set => "set",
            Heap => "heap",
            Array => "array",
        }
    }

    /// True if this keyword names a built-in type.
    pub fn is_type(self) -> bool {
        use Keyword::*;
        matches!(
            self,
            Int8 | Int16
                | Int32
                | Int64
                | Float32
                | Float64
                | I8
                | I16
                | I32
                | I64
                | F32
                | F64
                | Uint8
                | Uint16
                | Uint32
                | Uint64
                | U8
                | U16
                | U32
                | U64
                | Usize
                | Bool
                | Char
                | Str
                | Bytes
                | Bigint
                | Complex
                | List
                | Dict
                | Set
                | Heap
                | Array
                | Channel
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_roundtrip() {
        for kw in [
            Keyword::Def,
            Keyword::Channel,
            Keyword::Comptime,
            Keyword::True,
            Keyword::Int64,
            Keyword::Array,
        ] {
            assert_eq!(Keyword::from_ident(kw.as_str()), Some(kw));
        }
    }

    #[test]
    fn soft_keywords_lex_as_identifiers() {
        // Contextual (soft) keywords are never reserved words, so the lexer
        // emits them as ordinary identifiers and the parser recognizes them
        // only in-slot. Tier 1 (#1405/#1411): match/case/select/scope. Tier 2
        // (#1412): spawn/when/do/unsafe/python.
        for w in [
            "match", "case", "select", "scope", // Tier 1
            "spawn", "when", "do", "unsafe", "python", // Tier 2
        ] {
            assert_eq!(Keyword::from_ident(w), None, "`{w}` must not be reserved");
        }
    }

    #[test]
    fn python_casing_and_contextual() {
        // Full Python casing (D-0041): capitalized value keywords are reserved,
        // lowercase ones are not.
        assert_eq!(Keyword::from_ident("True"), Some(Keyword::True));
        assert_eq!(Keyword::from_ident("None"), Some(Keyword::None));
        assert_eq!(Keyword::from_ident("true"), None);
        // `m`/`s` are contextual regex prefixes, not reserved words.
        assert_eq!(Keyword::from_ident("m"), None);
        assert_eq!(Keyword::from_ident("s"), None);
        // Ordinary identifiers are not keywords.
        assert_eq!(Keyword::from_ident("fib"), None);
    }

    #[test]
    fn type_keywords() {
        assert!(Keyword::Int64.is_type());
        assert!(Keyword::Array.is_type());
        assert!(!Keyword::Def.is_type());
    }
}
