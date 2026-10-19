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
    Comma,        // ,
    Dot,          // .
    Ellipsis,     // ...  (variadic, D-0038)
    At,           // @    (decorator, D-0036)
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
    // Declarations / control flow
    Def,
    Return,
    If,
    Elif,
    Else,
    While,
    Do,
    For,
    In,
    Range,
    Match,
    Case,
    Break,
    Continue,
    Auto,
    Const,
    Raise,
    Try,
    Except,
    Finally,
    With,
    As,
    Yield,
    Lambda,
    Import,
    Python,
    // Concurrency
    Spawn,
    Channel,
    Select,
    Close,
    Scope,
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
    // Built-in type names
    Int32,
    Int64,
    Float32,
    Float64,
    I32,
    I64,
    F32,
    F64,
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
            "do" => Do,
            "for" => For,
            "in" => In,
            "range" => Range,
            "match" => Match,
            "case" => Case,
            "break" => Break,
            "continue" => Continue,
            "auto" => Auto,
            "const" => Const,
            "raise" => Raise,
            "try" => Try,
            "except" => Except,
            "finally" => Finally,
            "with" => With,
            "as" => As,
            "yield" => Yield,
            "lambda" => Lambda,
            "import" => Import,
            "python" => Python,
            "spawn" => Spawn,
            "channel" => Channel,
            "select" => Select,
            "close" => Close,
            "scope" => Scope,
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
            "int32" => Int32,
            "int64" => Int64,
            "float32" => Float32,
            "float64" => Float64,
            "i32" => I32,
            "i64" => I64,
            "f32" => F32,
            "f64" => F64,
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
            Do => "do",
            For => "for",
            In => "in",
            Range => "range",
            Match => "match",
            Case => "case",
            Break => "break",
            Continue => "continue",
            Auto => "auto",
            Const => "const",
            Raise => "raise",
            Try => "try",
            Except => "except",
            Finally => "finally",
            With => "with",
            As => "as",
            Yield => "yield",
            Lambda => "lambda",
            Import => "import",
            Python => "python",
            Spawn => "spawn",
            Channel => "channel",
            Select => "select",
            Close => "close",
            Scope => "scope",
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
            Int32 => "int32",
            Int64 => "int64",
            Float32 => "float32",
            Float64 => "float64",
            I32 => "i32",
            I64 => "i64",
            F32 => "f32",
            F64 => "f64",
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
            Int32
                | Int64
                | Float32
                | Float64
                | I32
                | I64
                | F32
                | F64
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
            Keyword::Spawn,
            Keyword::Match,
            Keyword::True,
            Keyword::Int64,
            Keyword::Array,
        ] {
            assert_eq!(Keyword::from_ident(kw.as_str()), Some(kw));
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
