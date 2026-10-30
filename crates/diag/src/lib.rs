//! Zap's stable error-code scheme — `ZAPnnnn` (I#157/F1, issue #239).
//!
//! Every diagnostic the front-end (and, later, the back end) emits carries a
//! stable [`ErrorCode`]. The code — not the human-readable message — is the
//! machine-parseable, compatibility-governed identifier a tool or coding agent
//! keys off. This is the pillar-1 multiplier: a stable code lets an agent map a
//! failure to a fix without parsing prose that may change between releases.
//!
//! # The scheme
//!
//! A code is `ZAP` + a four-digit number. The leading digit is the **category**
//! (roughly, the compiler phase that produces it):
//!
//! | Range          | Category    | Phase |
//! |----------------|-------------|-------|
//! | `ZAP0001`–`0999` | meta        | internal / cross-cutting |
//! | `ZAP1xxx`        | lexical     | lex |
//! | `ZAP2xxx`        | syntax      | parse |
//! | `ZAP3xxx`        | name        | sema (resolution / imports) |
//! | `ZAP4xxx`        | type        | sema (type checking) |
//! | `ZAP5xxx`        | semantic    | sema (declaration / rule validation) |
//! | `ZAP6xxx`        | backend     | lowering / IR / codegen |
//! | `ZAP7xxx`–`9xxx` | *reserved*  | future phases (linker, const-eval, …) |
//!
//! Severity (error vs warning) is **orthogonal** and is carried by the
//! diagnostic, not encoded in the code: a code names a *condition*, which could
//! in principle be reported at either severity.
//!
//! # Stability
//!
//! The scheme is a public tooling surface governed by `docs/STABILITY.md`. Its
//! contract, in one line: a code's *documented meaning* is stable within a
//! MAJOR; *adding* codes is a MINOR change; *renumbering, removing or
//! repurposing* a stable code is breaking; and the *message text* is never
//! stable — tools key off the code. Pre-1.0 (the current 0.x regime) makes no
//! promise, and every code ships **Experimental** until the #206 spec-freeze
//! audit ratifies the stable set.
//!
//! # Single source of truth
//!
//! The [`define_codes!`] table below generates *both* the typed associated
//! constants (so emission sites are compile-checked — you cannot emit a code
//! that is not in the table) *and* the [`REGISTRY`] that backs `zap explain`
//! and documentation. A test asserts the table has no duplicate numbers.

#![forbid(unsafe_code)]

use core::fmt;

/// A stable diagnostic identifier, rendered `ZAPnnnn`.
///
/// The inner number is private: the only way to name a code is through a
/// generated associated constant (e.g. [`ErrorCode::ZAP4001`]) or [`parse`],
/// both of which are guaranteed to be present in the [`REGISTRY`]. That is what
/// makes "every emitted code is a registered code" a compile-time property.
///
/// [`parse`]: ErrorCode::parse
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ErrorCode(u16);

/// The category of a code, derived from its leading digit. Purely a convenience
/// for grouping and triage; the code itself is the stable identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// `ZAP0xxx` — internal or cross-cutting (ICE, not-yet-implemented).
    Meta,
    /// `ZAP1xxx` — lexing.
    Lexical,
    /// `ZAP2xxx` — parsing.
    Syntax,
    /// `ZAP3xxx` — name resolution and imports.
    Name,
    /// `ZAP4xxx` — type checking.
    Type,
    /// `ZAP5xxx` — declaration and semantic-rule validation.
    Semantic,
    /// `ZAP6xxx` — lowering, IR, and codegen.
    Backend,
    /// `ZAP7xxx`–`ZAP9xxx` — reserved for future phases.
    Reserved,
}

impl Category {
    /// A short, stable, lower-case slug (used by `zap explain` and docs).
    pub fn slug(self) -> &'static str {
        match self {
            Category::Meta => "meta",
            Category::Lexical => "lexical",
            Category::Syntax => "syntax",
            Category::Name => "name",
            Category::Type => "type",
            Category::Semantic => "semantic",
            Category::Backend => "backend",
            Category::Reserved => "reserved",
        }
    }
}

/// One entry in the [`REGISTRY`]: a code plus the human-facing text that
/// `zap explain` prints. The `title` is a short noun phrase; the `explanation`
/// is one or two sentences saying what the condition is and how to fix it.
#[derive(Clone, Copy, Debug)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    pub title: &'static str,
    pub explanation: &'static str,
}

impl ErrorInfo {
    /// The code's category.
    pub fn category(&self) -> Category {
        self.code.category()
    }
}

impl ErrorCode {
    /// The bare number (e.g. `4001`).
    pub const fn number(self) -> u16 {
        self.0
    }

    /// This code's category, from its leading digit.
    pub const fn category(self) -> Category {
        match self.0 / 1000 {
            0 => Category::Meta,
            1 => Category::Lexical,
            2 => Category::Syntax,
            3 => Category::Name,
            4 => Category::Type,
            5 => Category::Semantic,
            6 => Category::Backend,
            _ => Category::Reserved,
        }
    }

    /// The registry entry for this code (always `Some` for a code obtained from
    /// a constant or [`parse`](ErrorCode::parse)).
    pub fn info(self) -> Option<&'static ErrorInfo> {
        REGISTRY.iter().find(|e| e.code == self)
    }

    /// Parse a `ZAPnnnn` string into a registered code. Returns `None` if the
    /// string is malformed or names a number that is not in the registry, so a
    /// caller (e.g. `zap explain`) can report an unknown code cleanly. The
    /// `ZAP` prefix is matched case-insensitively.
    pub fn parse(s: &str) -> Option<ErrorCode> {
        let s = s.trim();
        let digits = s
            .strip_prefix("ZAP")
            .or_else(|| s.strip_prefix("zap"))
            .or_else(|| s.strip_prefix("Zap"))?;
        // Reject anything that is not exactly the numeric tail (no signs, no
        // whitespace, no leading `+`), but accept any digit count the registry
        // might use.
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: u16 = digits.parse().ok()?;
        let code = ErrorCode(n);
        code.info().map(|_| code)
    }
}

impl fmt::Display for ErrorCode {
    /// Renders `ZAPnnnn`, zero-padded to four digits.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ZAP{:04}", self.0)
    }
}

impl fmt::Debug for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ErrorCode({self})")
    }
}

/// Declare the code table once; generate the typed constants and the registry
/// from it. Each row is `NAME = number : "title", "explanation";`. The category
/// is derived from the number, so it is never stated twice.
macro_rules! define_codes {
    ($( $(#[$m:meta])* $name:ident = $num:literal : $title:literal , $explain:literal ; )+) => {
        impl ErrorCode {
            $( $(#[$m])* #[doc = $title] pub const $name: ErrorCode = ErrorCode($num); )+
        }

        /// Every registered code, in declaration order. The single source of
        /// truth for `zap explain` and generated documentation.
        pub static REGISTRY: &[ErrorInfo] = &[
            $( ErrorInfo { code: ErrorCode($num), title: $title, explanation: $explain }, )+
        ];
    };
}

define_codes! {
    // ---- ZAP0xxx — meta / cross-cutting ------------------------------------
    ZAP0001 = 1 : "internal compiler error",
        "The compiler hit a state it believes is impossible. This is a bug in \
         Zap, not in your program — please report it with a minimal reproducer.";
    ZAP0002 = 2 : "feature not yet implemented",
        "The construct is valid Zap but this toolchain version does not support \
         it yet. Check the issue tracker for the tracking ticket.";

    // ---- ZAP1xxx — lexical -------------------------------------------------
    ZAP1001 = 1001 : "unterminated string literal",
        "A string literal was opened but never closed before end of line/file. \
         Add the closing quote.";
    ZAP1002 = 1002 : "unterminated regex literal",
        "A regex literal (`/.../` or `m/.../`) was opened but never closed. Add \
         the closing delimiter.";
    ZAP1003 = 1003 : "unterminated substitution literal",
        "A substitution literal (`s/.../.../`) was opened but never closed. Add \
         the closing delimiter.";
    ZAP1004 = 1004 : "missing substitution delimiter",
        "An `s{...}{...}` substitution is missing the replacement part. Provide \
         both the pattern and the replacement.";
    ZAP1005 = 1005 : "inconsistent indentation",
        "A line's indentation does not match any enclosing block. Align it with \
         one of the open indentation levels.";
    ZAP1006 = 1006 : "unexpected character",
        "A character that is not part of any Zap token appeared in the source. \
         Remove it or fix the surrounding token.";
    ZAP1007 = 1007 : "stray `!`",
        "`!` is not a Zap operator. Use `!=` for inequality or `not` for logical \
         negation.";
    ZAP1008 = 1008 : "tab in indentation",
        "Zap indentation must use spaces, not tabs. Replace the leading tab(s) \
         with spaces.";

    // ---- ZAP2xxx — syntax --------------------------------------------------
    ZAP2001 = 2001 : "unexpected token",
        "The parser expected a particular token or construct here and found \
         something else. The message names what was expected.";
    ZAP2002 = 2002 : "unexpected token at top level",
        "Only declarations (functions, types, imports, …) may appear at the top \
         level of a module.";
    ZAP2003 = 2003 : "misplaced decorator",
        "A decorator (`@name`) must immediately precede the declaration it \
         applies to.";
    ZAP2004 = 2004 : "unexpected token in struct body",
        "A struct body contains only field declarations and methods. Remove or \
         fix the offending token.";
    ZAP2005 = 2005 : "invalid bitfield width",
        "A bitfield width must be written as a plain decimal integer, e.g. \
         `: 3`.";
    ZAP2006 = 2006 : "missing bitfield width",
        "A `:` in a bitfield field must be followed by an integer width, e.g. \
         `flags: int32 : 3`.";
    ZAP2007 = 2007 : "expected a string literal",
        "This position requires a string literal.";
    ZAP2008 = 2008 : "unexpected token in block",
        "A statement block contains something that is not a statement. Check for \
         a stray token or missing keyword.";
    ZAP2009 = 2009 : "expected a newline",
        "Statements are newline-terminated; a second statement cannot share a \
         line without an explicit separator.";
    ZAP2010 = 2010 : "expected a type",
        "A type is required here (e.g. after `:` or `->`).";
    ZAP2011 = 2011 : "expected a regex literal",
        "The `=~` / `!~` match operators require a regex literal on the right.";
    ZAP2012 = 2012 : "expected a substitution literal",
        "The `~=` operator requires a substitution literal (`s/.../.../`) on the \
         right.";
    ZAP2013 = 2013 : "expected an index expression",
        "An indexing `[...]` must contain an index expression.";
    ZAP2014 = 2014 : "expected an expression",
        "An expression is required here and none was found.";
    ZAP2015 = 2015 : "invalid `when` target predicate",
        "A `when target.*` arm must test the build target in one of the v1 \
         forms: `target.arch == .<arch>`, `target.os == .<os>`, or \
         `target.has(.<feature>)` (I#139, D-0079). Boolean combinators are not \
         supported yet.";
    ZAP2016 = 2016 : "unknown target name in `when`",
        "The architecture or OS named in a `when target.*` predicate is not one \
         the compiler knows. v1 accepts `.x86_64`/`.aarch64` for `target.arch` \
         and `.linux`/`.windows`/`.macos` for `target.os`.";
    ZAP2017 = 2017 : "malformed f-string",
        "An f-string replacement field is ill-formed (D-0026): an unmatched \
         `{` or `}` (write `{{`/`}}` for a literal brace), an empty `{}`, or a \
         field whose contents are not a single valid expression.";

    // ---- ZAP3xxx — name resolution / imports -------------------------------
    ZAP3001 = 3001 : "unknown name",
        "The name is not defined in any enclosing scope. Check the spelling, or \
         declare or import it before use.";
    ZAP3002 = 3002 : "assignment to undeclared variable",
        "Assigning to a name that was never declared. Introduce it with a \
         declaration (e.g. `x = ...` at its first use, or a typed `let`).";
    ZAP3003 = 3003 : "call to unknown function",
        "The called function is not defined or imported. Check the spelling or \
         add the definition/import.";
    // Zap-to-Zap module & package imports (I#283, D-0081).
    ZAP3010 = 3010 : "module not found",
        "An `import` names a Zap module that could not be resolved to a file. A \
         module `a` resolves to `<src>/a.zap` and `a.b` to `<src>/a/b.zap` \
         (D-0081). Check the path and that the file exists.";
    ZAP3011 = 3011 : "circular import",
        "Two or more modules import each other, forming a cycle. v1 rejects \
         import cycles (D-0081); break the cycle, e.g. by moving the shared \
         declarations into a third module both import.";
    ZAP3012 = 3012 : "access to a private module member",
        "A name beginning with `_` is private to its defining module and cannot \
         be accessed through an import (D-0028, enforced). Remove the leading \
         underscore to export it, or access it only from within its module.";
    ZAP3013 = 3013 : "no such public member in module",
        "The imported module has no public top-level declaration with this name. \
         Check the spelling, or that the declaration exists and is not private \
         (a leading `_`).";
    ZAP3014 = 3014 : "top-level statement in an imported module",
        "Only the entry module runs top-level statements; an imported module is \
         declaration-only (D-0081). Move executable code into a function the \
         importer calls.";
    ZAP3015 = 3015 : "module name used as a value",
        "An imported module's name is not a value — use it only to qualify a \
         member access (`m.item`), not on its own.";

    // ---- ZAP4xxx — type checking -------------------------------------------
    ZAP4001 = 4001 : "type mismatch in assignment",
        "The value's type is not assignable to the target's declared type. \
         Convert the value or adjust the declared type.";
    ZAP4002 = 4002 : "type mismatch in augmented assignment",
        "An augmented assignment (`+=`, `-=`, …) produced a type that does not \
         match the variable's type.";
    ZAP4003 = 4003 : "return type mismatch",
        "A `return` value's type does not match the function's declared return \
         type.";
    ZAP4004 = 4004 : "missing return value",
        "A function with a declared return type reached a `return` with no \
         value, or an end of body, where a value is required.";
    ZAP4010 = 4010 : "unary operator needs a number",
        "Unary `+`/`-` require a numeric operand.";
    ZAP4011 = 4011 : "`~` needs an integer",
        "Bitwise-not `~` requires an integer operand.";
    ZAP4012 = 4012 : "arithmetic needs numbers",
        "A binary arithmetic operator was applied to a non-numeric operand (or \
         two types with no common numeric type).";
    ZAP4013 = 4013 : "division needs numbers",
        "`/` / `//` / `%` require numeric operands.";
    ZAP4014 = 4014 : "bitwise operator needs integers",
        "Bitwise operators (`&`, `|`, `^`, `<<`, `>>`) require integer \
         operands.";
    ZAP4020 = 4020 : "wrong number of arguments",
        "A call passed too few or too many arguments for the callee's arity.";
    ZAP4021 = 4021 : "argument type mismatch",
        "A positional argument's type does not match the parameter's type.";
    ZAP4022 = 4022 : "keyword argument type mismatch",
        "A keyword argument's type does not match the parameter's type.";
    ZAP4023 = 4023 : "unknown parameter",
        "A keyword argument names a parameter the function does not have. Check \
         the spelling.";
    ZAP4024 = 4024 : "built-in argument type mismatch",
        "A built-in was called with an argument of the wrong type.";
    ZAP4026 = 4026 : "invalid format spec",
        "A format spec (`{expr:spec}` in an f-string, or the second argument to \
         `format`) is not valid for the value's type, or uses a feature this \
         toolchain does not support (D-0026). v1 supports the common Python \
         mini-language: `[[fill]align][sign][#][0][width][.precision][type]`, \
         with integer types `b/o/x/X/d`, float types `e/f/g`, and `s` for \
         strings; the `!r`/`!a` conversions require the Display protocol (#274).";
    ZAP4025 = 4025 : "wrong number of arguments to built-in",
        "A built-in was called with the wrong number of arguments.";
    ZAP4030 = 4030 : "receive needs a channel",
        "The `<-` receive operator requires a channel operand.";
    ZAP4031 = 4031 : "send needs a channel",
        "The left operand of a `<-` send must be a channel.";
    ZAP4032 = 4032 : "channel send type mismatch",
        "The sent value's type does not match the channel's element type.";
    ZAP4033 = 4033 : "invalid channel capacity",
        "A channel's buffer capacity must be an integer.";
    ZAP4040 = 4040 : "match operand must be a string",
        "The left operand of `=~` / `!~` must be a `str`.";
    ZAP4041 = 4041 : "substitution operand must be a string",
        "The left operand of a substitution (`s/.../.../`) must be a `str`.";
    ZAP4050 = 4050 : "tuple index out of range",
        "A tuple index refers past the end of the tuple.";
    ZAP4051 = 4051 : "non-constant tuple index",
        "A tuple must be indexed by a compile-time integer constant, because \
         each element may have a different type.";
    ZAP4060 = 4060 : "unknown enum variant",
        "The enum has no variant with this name. Check the spelling against the \
         enum's declaration.";
    ZAP4061 = 4061 : "variant carries data",
        "This variant carries data and cannot be used as a bare value; \
         construct it with its fields.";
    ZAP4062 = 4062 : "wrong number of variant fields",
        "A data-carrying variant was constructed with the wrong number of \
         fields.";
    ZAP4063 = 4063 : "unknown variant field",
        "A variant field name does not exist on the variant.";
    ZAP4064 = 4064 : "variant field type mismatch",
        "A value given for a variant field has the wrong type.";
    ZAP4070 = 4070 : "type is not iterable",
        "A value used in a `for`/iteration context is of a type that defines no \
         `__iter__`.";
    ZAP4071 = 4071 : "operator not supported",
        "A type does not define the dunder required for this operator.";
    ZAP4072 = 4072 : "`in` not supported",
        "A type does not define `__contains__`, so `in` cannot be used with it.";
    ZAP4080 = 4080 : "no such method",
        "An interface or a type parameter's bound does not declare the method \
         being called.";

    // ---- ZAP5xxx — declaration / semantic-rule validation ------------------
    ZAP5001 = 5001 : "recursive value struct",
        "A `value struct` cannot contain itself by value (that would need \
         infinite storage). Use a reference, or an indirection.";
    ZAP5010 = 5010 : "bit width on a non-value-struct field",
        "Bit widths are only allowed on `value struct` fields (D-0068).";
    ZAP5011 = 5011 : "incomplete bitfield",
        "Every field of a bitfield `value struct` must declare a bit width.";
    ZAP5012 = 5012 : "bitfield width too small",
        "A bitfield must be at least 1 bit wide.";
    ZAP5013 = 5013 : "bitfield width exceeds type",
        "A bitfield is wider than the bits its declared type can hold.";
    ZAP5014 = 5014 : "bitfield too narrow for enum",
        "A bitfield is too narrow to represent all of its enum's \
         discriminants.";
    ZAP5015 = 5015 : "bitfield struct exceeds machine word",
        "A bitfield `value struct` packs more bits than one 64-bit machine \
         word.";
    ZAP5016 = 5016 : "reference enum in bitfield",
        "A tagged (reference) enum cannot be packed into a bitfield.";
    ZAP5017 = 5017 : "negative discriminant in bitfield enum",
        "An enum with a negative discriminant cannot be packed into a \
         bitfield.";
    ZAP5018 = 5018 : "invalid bitfield type",
        "A bitfield must be a fixed-width integer, `bool`, or a tier-1 enum — \
         not a pointer/reference, float, char, or bigint (D-0068).";
    ZAP5030 = 5030 : "unknown annotation field",
        "An annotation was given a field it does not define.";
    ZAP5031 = 5031 : "too many annotation arguments",
        "An annotation was given more arguments than it accepts.";
    ZAP5032 = 5032 : "unknown decorator or annotation",
        "The `@name` is neither a built-in decorator/annotation nor a \
         user-defined one in scope.";
    ZAP5033 = 5033 : "empty @derive",
        "`@derive(...)` needs at least one trait name, e.g. `@derive(Eq, \
         Repr)`.";
    ZAP5034 = 5034 : "@derive takes positional names",
        "`@derive` takes positional trait names, not keyword arguments.";
    ZAP5035 = 5035 : "unknown derivable trait",
        "`@derive` was given a trait it cannot derive.";
    ZAP5036 = 5036 : "@derive expects trait names",
        "`@derive` arguments must be trait names, e.g. `@derive(Eq)`.";
    ZAP5037 = 5037 : "invalid @on_signal argument",
        "`@on_signal` takes exactly one `Signal` member, e.g. \
         `@on_signal(Signal.SIGINT)`.";
    ZAP5038 = 5038 : "invalid @default argument",
        "`@default(expr)` takes exactly one expression.";
    ZAP5039 = 5039 : "invalid @range bounds",
        "`@range(...)` takes keyword bounds `min`/`max`, at least one of them.";
    ZAP5040 = 5040 : "invalid @validate argument",
        "`@validate(...)` takes keyword constraints, e.g. `@validate(max_len=50)`.";
    ZAP5041 = 5041 : "decorator takes no arguments",
        "This decorator accepts no arguments.";
    ZAP5042 = 5042 : "invalid @deprecated argument",
        "`@deprecated` takes an optional message string.";
    ZAP5043 = 5043 : "invalid @skip argument",
        "`@skip` takes an optional reason string.";
    ZAP5044 = 5044 : "invalid @skip_if arguments",
        "`@skip_if` takes a condition and an optional reason.";
    ZAP5045 = 5045 : "invalid @expect_raises argument",
        "`@expect_raises` takes exactly one exception type.";
    ZAP5046 = 5046 : "invalid @parametrize argument",
        "`@parametrize` takes a single `cases` list.";
    ZAP5047 = 5047 : "decorator on wrong target",
        "This decorator does not apply to the kind of declaration it was placed \
         on.";
    ZAP5060 = 5060 : "generator enum method not supported",
        "Generators as enum methods are not supported yet (I#429 extended the \
         struct-method case only); define the generator as a free function or a \
         struct method.";
    ZAP5061 = 5061 : "`main` cannot be a generator",
        "The entry point `main` must not contain `yield`.";
    ZAP5062 = 5062 : "generator must not declare a return type",
        "A generator returns an iterator over its yielded values, so it must not \
         declare a return type (I#67, D-0048).";
    ZAP5063 = 5063 : "generator return cannot carry a value",
        "In this version a generator's `return` must be bare; use plain \
         `return` to end iteration (I#67 v1).";
    ZAP5064 = 5064 : "`yield` requires a value",
        "In this version `yield` must carry a value (I#67 v1).";
    ZAP5070 = 5070 : "invalid scope on_error",
        "`scope(on_error=...)` expects `OnError.Cancel` or `OnError.Wait`.";
    ZAP5071 = 5071 : "scope timeout not supported",
        "`scope(timeout=...)` is not supported yet (the timer API is deferred).";
    ZAP5072 = 5072 : "invalid scope cancel_on",
        "`scope(cancel_on=...)` expects a list of `Signal`.";
    ZAP5073 = 5073 : "invalid scope grace",
        "`scope(grace=...)` must be an integer number of milliseconds.";
    ZAP5074 = 5074 : "unknown scope option",
        "`scope()` was given an option it does not recognise.";
    ZAP5075 = 5075 : "scope options must be keywords",
        "`scope()` options must be keyword arguments, e.g. \
         `on_error=OnError.Wait`.";
    ZAP5080 = 5080 : "non-exhaustive match",
        "A `match` on an enum does not cover every variant. Add the missing \
         arm(s) or a wildcard `_`.";
    ZAP5081 = 5081 : "pattern type mismatch",
        "An enum pattern cannot match a value of the scrutinee's type.";
    ZAP5082 = 5082 : "wrong field count in pattern",
        "A variant pattern binds a different number of fields than the variant \
         has.";
    ZAP5090 = 5090 : "invalid declaration target",
        "A declaration target must be a name (or a tuple of names).";
    ZAP5091 = 5091 : "misplaced `*rest`",
        "`*rest` is only valid as an element of a tuple destructuring target.";
    ZAP5092 = 5092 : "cannot destructure",
        "The value's type cannot be destructured with this pattern.";
    ZAP5093 = 5093 : "multiple `*rest` in target",
        "A tuple target may have at most one `*rest` element.";
    ZAP5094 = 5094 : "tuple destructuring arity mismatch",
        "The number of elements in the pattern does not match the tuple.";
    ZAP5100 = 5100 : "invalid heap cmp value",
        "`heap[T](cmp=...)` must be the string `\"min\"` or `\"max\"`.";
    ZAP5101 = 5101 : "heap cmp must be a literal",
        "`heap[T](cmp=...)` must be a string literal, not a computed value.";
    ZAP5102 = 5102 : "unknown heap keyword",
        "`heap[T](...)` has only the keyword argument `cmp`.";
    ZAP5103 = 5103 : "invalid heap arguments",
        "`heap[T](...)` takes only the keyword argument `cmp`.";
    ZAP5110 = 5110 : "sum type needs a variant",
        "A sum type cannot be constructed directly; construct one of its \
         variants.";
    ZAP5120 = 5120 : "equality without `__hash__`",
        "A type defines equality but no `__hash__`, yet is used as a dict/set \
         key; its hashing will not agree with its equality (D-0047).";

    // ---- ZAP6xxx — lowering / IR / codegen (Experimental) ------------------
    ZAP6001 = 6001 : "lowering error",
        "The front-end accepted the program but it could not be lowered to IR. \
         The message gives the specific reason.";

    // ---- ZAP6xxx — package registry (Experimental, D-0070 §4; I#236) -------
    // The registry client is a host-side (back-end-category) tool surface; its
    // conditions carry stable codes so an agent can map a failure to a fix
    // without parsing prose (D-0071). Codes name conditions; severity is on the
    // diagnostic (e.g. ZAP6102 is a warning).
    ZAP6100 = 6100 : "package checksum mismatch",
        "A downloaded registry package's content hash does not match the \
         `zap1:` checksum pinned in the index or the lockfile. The package was \
         discarded; the bytes have been tampered with or corrupted, or the \
         lockfile pins a different version's content.";
    ZAP6101 = 6101 : "package archive digest mismatch",
        "A downloaded registry archive's byte digest does not match the one the \
         index records. This is the cheap pre-unpack transfer check; re-fetch \
         the archive, which may have been truncated or corrupted in transit.";
    ZAP6102 = 6102 : "yanked version selected",
        "Resolution selected a version that has been yanked from the registry. \
         A yanked version still builds for an existing lock, but a fresh \
         resolve should raise the requirement's minimum to an unyanked version.";
    ZAP6103 = 6103 : "toolchain too old for package",
        "A registry package declares a `min_zap` toolchain version newer than \
         the one in use. Upgrade the Zap toolchain, or pin an earlier version \
         of the package whose `min_zap` this toolchain satisfies.";
    ZAP6104 = 6104 : "registry package name conflict",
        "A registry package name is invalid, reserved, or normalizes to the \
         same identity as another dependency (names fold case and `-`/`_`, so \
         `json-lib` and `json_lib` are the same). Rename or deduplicate it.";
    ZAP6105 = 6105 : "malformed or unreachable registry index",
        "A registry's `config.json` or a package index file could not be \
         fetched or parsed, or a requested version is absent from the index. \
         Check the index base URL and network access.";

    // ---- `zap publish` write path (Experimental, D-0083; I#237) -----------
    ZAP6106 = 6106 : "package version already published",
        "`zap publish` refused to overwrite a `(name, version)` that the index \
         already records. Published versions are immutable (D-0027 §4): bump \
         the `[package].version` and publish again, or `yank` the old one.";
    ZAP6107 = 6107 : "incomplete package for publish",
        "`zap publish` requires a well-formed `[package]` table: a `name` that \
         is a valid, non-reserved registry name and a `version` that parses as \
         SemVer. Fix `zap.toml` and retry.";
    ZAP6108 = 6108 : "registry publish target unavailable",
        "`zap publish` can write to a static index served from a local path or \
         `file://` URL, but the chosen target needs the hosted, authenticated \
         write API (an `https://` index), which is not yet deployed (gated on \
         the org move, D-0027 §5.2/§7). Publish to a local/`file://` index, or \
         point `--index`/`ZAP_REGISTRY` at one.";
    ZAP6109 = 6109 : "missing or invalid registry credentials",
        "A registry operation needs an authentication token (`zap login`), or \
         the stored token is empty/unreadable. Tokens live outside the repo in \
         `$ZAP_CACHE/credentials` and are only consumed by the hosted write \
         API; a local/`file://` index needs none.";
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn registry_has_no_duplicate_numbers() {
        let mut seen = BTreeSet::new();
        for e in REGISTRY {
            assert!(
                seen.insert(e.code.number()),
                "duplicate error-code number {}",
                e.code
            );
        }
    }

    #[test]
    fn registry_is_non_empty_and_all_renders_are_well_formed() {
        assert!(!REGISTRY.is_empty());
        for e in REGISTRY {
            let s = e.code.to_string();
            assert!(s.starts_with("ZAP"));
            assert_eq!(s.len(), 7, "code {s} is not ZAP + 4 digits");
            assert!(!e.title.is_empty());
            assert!(!e.explanation.is_empty());
            // Round-trips through parse.
            assert_eq!(ErrorCode::parse(&s), Some(e.code));
        }
    }

    #[test]
    fn category_from_leading_digit() {
        assert_eq!(ErrorCode::ZAP0001.category(), Category::Meta);
        assert_eq!(ErrorCode::ZAP1001.category(), Category::Lexical);
        assert_eq!(ErrorCode::ZAP2001.category(), Category::Syntax);
        assert_eq!(ErrorCode::ZAP3001.category(), Category::Name);
        assert_eq!(ErrorCode::ZAP4001.category(), Category::Type);
        assert_eq!(ErrorCode::ZAP5001.category(), Category::Semantic);
        assert_eq!(ErrorCode::ZAP6001.category(), Category::Backend);
    }

    #[test]
    fn parse_rejects_garbage_and_unregistered() {
        assert_eq!(ErrorCode::parse(""), None);
        assert_eq!(ErrorCode::parse("ZAP"), None);
        assert_eq!(ErrorCode::parse("ZAPxyz"), None);
        assert_eq!(ErrorCode::parse("4001"), None); // missing prefix
        assert_eq!(ErrorCode::parse("ZAP9999"), None); // not registered
        assert_eq!(ErrorCode::parse("zap4001"), Some(ErrorCode::ZAP4001));
        assert_eq!(ErrorCode::parse("  ZAP4001  "), Some(ErrorCode::ZAP4001));
    }
}
