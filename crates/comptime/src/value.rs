//! The Tier-B1 comptime value model (#1250).
//!
//! A [`Value`] is what a comptime expression evaluates to. B1 covers the scalar
//! types `int64`/`bool`/`str`/`char`, homogeneous-or-not `list`/`tuple`
//! aggregates of comptime values, and the **read-only E1 reflection records**
//! (`TypeInfo`/`FieldInfo`/`VariantInfo`/`AttrInfo`/`AttrArg` and the
//! `TypeKind` tag) — reused verbatim from [`zapc_sema_types`], never redefined.
//!
//! Floats, bigint, `bytes`, `None`/optionals, dict/set and user-struct runtime
//! values are **deliberately absent**: reaching for one is an out-of-fragment
//! error (`… is not available at comptime`), which is how the evaluator stays a
//! sound physical backstop (epic #1249, locked decision 2b).
//!
//! Aggregates and records are wrapped in [`Rc`] so a `Value` is cheap to clone
//! (comptime values are immutable in B1 — there is no in-place mutation op), and
//! so a reflection record can flow through the evaluator without being copied.

use std::fmt;
use std::rc::Rc;

use zapc_sema_types::{AttrArg, AttrInfo, FieldInfo, TypeInfo, TypeKind, VariantInfo};
use zapc_syntax::ast::Type as AstType;
use zapc_syntax::span::Span;

/// A compile-time value in the Tier-B1 fragment (#1250).
#[derive(Clone)]
pub enum Value {
    /// A 64-bit signed integer. The *only* numeric type in B1 — its arithmetic
    /// mirrors runtime `int64` exactly (two's-complement wrap, Python-floor
    /// `//`/`%`); float and bigint are deferred to B2.
    Int(i64),
    /// A boolean.
    Bool(bool),
    /// A UTF-8 string. Indexed and sliced by **codepoint** (D-0025), matching
    /// the runtime `str`.
    Str(Rc<str>),
    /// A single Unicode scalar value (`char`), e.g. the result of `s[i]` or
    /// `char(n)`.
    Char(char),
    /// A `list` of comptime values (immutable in B1).
    List(Rc<Vec<Value>>),
    /// A `tuple` of comptime values (heterogeneous, fixed arity, immutable).
    Tuple(Rc<Vec<Value>>),
    /// An E1 `TypeInfo` reflection record (`reflect.type_info(T)`).
    ///
    /// The second component is the **use-site** declared [`AstType`] this
    /// reference came from, when known (#1346): the `list[Widget]` in
    /// `list[Widget] items`, or the nested `Widget` reached via `.elem`. It is
    /// the per-reference source location — distinct from the canonical,
    /// memoized `TypeInfo`, which is structural and span-free, so the same type
    /// referenced at two declarations yields two values with two use-sites. It
    /// is `None` for a type resolved from a span-free resolved `Ty` (no AST in
    /// hand) and for a top-level `reflect.type_info(T)`. `.span` reads it, and
    /// the container accessors (`.elem`/`.key`/`.value`/`.inner`/`.elems`)
    /// descend into its argument subtree so a nested type's `.span` is its
    /// use-site, not a declaration elsewhere.
    Type(Rc<TypeInfo>, Option<Rc<AstType>>),
    /// An E1 `FieldInfo` record — a struct field or enum-variant payload field.
    Field(Rc<FieldInfo>),
    /// An E1 `VariantInfo` record — one enum variant.
    Variant(Rc<VariantInfo>),
    /// An E1 `AttrInfo` record — one `@attr(...)` recorded on a field.
    Attr(Rc<AttrInfo>),
    /// One argument of an [`Value::Attr`] (`AttrArg`), positional or keyword.
    AttrArg(Rc<AttrArg>),
    /// A `TypeKind` tag (the `kind` of a [`Value::Type`]).
    Kind(TypeKind),
    /// A source [`Span`] — the `.span` of a reflection record (a field,
    /// variant, or attribute). Its one use is as the `at` argument of
    /// `comptime.error(at, msg)` (#1256), so a generator can point a
    /// library-authored diagnostic at the user's offending declaration. It has
    /// no runtime representation (it never materializes into emitted code).
    Span(Span),
}

impl Value {
    /// A short, stable name for this value's type, used in diagnostics.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int64",
            Value::Bool(_) => "bool",
            Value::Str(_) => "str",
            Value::Char(_) => "char",
            Value::List(_) => "list",
            Value::Tuple(_) => "tuple",
            Value::Type(..) => "TypeInfo",
            Value::Field(_) => "FieldInfo",
            Value::Variant(_) => "VariantInfo",
            Value::Attr(_) => "AttrInfo",
            Value::AttrArg(_) => "AttrArg",
            Value::Kind(_) => "TypeKind",
            Value::Span(_) => "Span",
        }
    }

    /// The source span this value holds, if it is a [`Value::Span`].
    pub fn as_span(&self) -> Option<Span> {
        match self {
            Value::Span(s) => Some(*s),
            _ => None,
        }
    }

    /// Construct a [`Value::Str`] from anything string-like.
    pub fn str(s: impl Into<Rc<str>>) -> Value {
        Value::Str(s.into())
    }

    /// Construct a [`Value::List`] from a vec of values.
    pub fn list(items: Vec<Value>) -> Value {
        Value::List(Rc::new(items))
    }

    /// Construct a [`Value::Tuple`] from a vec of values.
    pub fn tuple(items: Vec<Value>) -> Value {
        Value::Tuple(Rc::new(items))
    }

    /// The boolean this value holds, if it is a [`Value::Bool`].
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The integer this value holds, if it is a [`Value::Int`].
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// The string slice this value holds, if it is a [`Value::Str`].
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// Structural equality for comptime values, for test convenience and for
/// equality of aggregate elements. Scalars and `list`/`tuple` aggregates compare
/// by value (recursively); the reflection records compare by **identity**
/// (`Rc::ptr_eq`) — the same underlying record is equal to itself, but two
/// independently-built records are not structurally diffed here (the comptime
/// `==` operator rejects comparing records anyway; see `ops::value_eq`).
/// `TypeKind` compares by its derived `Eq`.
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Tuple(a), Value::Tuple(b)) => a == b,
            (Value::Kind(a), Value::Kind(b)) => a == b,
            (Value::Span(a), Value::Span(b)) => a == b,
            // Two type references are equal when they name the same canonical
            // type record; the use-site span is a diagnostic attachment, not
            // part of a type's comptime identity.
            (Value::Type(a, _), Value::Type(b, _)) => Rc::ptr_eq(a, b),
            (Value::Field(a), Value::Field(b)) => Rc::ptr_eq(a, b),
            (Value::Variant(a), Value::Variant(b)) => Rc::ptr_eq(a, b),
            (Value::Attr(a), Value::Attr(b)) => Rc::ptr_eq(a, b),
            (Value::AttrArg(a), Value::AttrArg(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "Int({n})"),
            Value::Bool(b) => write!(f, "Bool({b})"),
            Value::Str(s) => write!(f, "Str({s:?})"),
            Value::Char(c) => write!(f, "Char({c:?})"),
            Value::List(xs) => write!(f, "List({xs:?})"),
            Value::Tuple(xs) => write!(f, "Tuple({xs:?})"),
            Value::Type(t, _) => write!(f, "Type({:?})", t.kind()),
            Value::Field(x) => write!(f, "Field({:?})", x.name),
            Value::Variant(x) => write!(f, "Variant({:?})", x.name),
            Value::Attr(x) => write!(f, "Attr({:?})", x.name),
            Value::AttrArg(x) => write!(f, "AttrArg({:?}={:?})", x.name, x.value),
            Value::Kind(k) => write!(f, "Kind({})", k.as_str()),
            Value::Span(s) => write!(f, "Span({}..{})", s.start, s.end),
        }
    }
}
