//! The pure comptime primitives — arithmetic, comparison, boolean and string
//! operations (#1250).
//!
//! **Numeric parity is the whole point** (issue #1250, epic #1249 locked
//! decision 5): every `int64` operation here produces *exactly* the bits the
//! runtime produces, so a value folded at comptime is indistinguishable from the
//! same computation run at runtime. The implementations mirror, line for line,
//! the runtime C helpers:
//!
//! - `+`/`-`/`*` wrap modulo 2^64 (two's complement, D-0019) — [`i64::wrapping_add`] etc.
//! - `//` is floor division and `%` is a floor-sign modulo, both special-casing
//!   `INT64_MIN` / `-1` to the wrapped result instead of a hardware trap —
//!   mirroring `zap_ifloordiv` / `zap_imod` (issue #742).
//! - `<<` / `>>` mirror `zap_ishl` / `zap_ishr`: a negative count is a fault, a
//!   count ≥ 64 saturates (`0`, or sign-fill for an arithmetic `>>`), and `>>`
//!   on a signed value is arithmetic.
//! - `str.upper()` / `.lower()` are **ASCII-only** byte case folds, mirroring
//!   `zap_str_upper` / `zap_str_lower`.
//! - slicing clamps its bounds Python-style (`zap_slice_pos`/`zap_slice_neg`)
//!   and faults only on a zero step; indexing faults when out of range.
//!
//! True division `/` (always float), `**`, `@`, bitwise/shift on non-`int64`,
//! and every wider numeric type are **out of the B1 fragment** and handled by
//! the evaluator as `not available at comptime`, not here.

use zapc_syntax::span::Span;

use crate::error::CtError;
use crate::value::Value;

// ---------------------------------------------------------------------------
// int64 arithmetic (runtime-parity)
// ---------------------------------------------------------------------------

/// `a + b`, wrapping (two's complement).
pub fn int_add(a: i64, b: i64) -> i64 {
    a.wrapping_add(b)
}

/// `a - b`, wrapping.
pub fn int_sub(a: i64, b: i64) -> i64 {
    a.wrapping_sub(b)
}

/// `a * b`, wrapping.
pub fn int_mul(a: i64, b: i64) -> i64 {
    a.wrapping_mul(b)
}

/// `a // b` — floor division, mirroring `zap_ifloordiv`.
pub fn int_floordiv(a: i64, b: i64, span: Span) -> Result<i64, CtError> {
    if b == 0 {
        return Err(CtError::value(
            "integer division or modulo by zero at comptime",
            span,
        ));
    }
    // INT64_MIN / -1 is unrepresentable and traps the hardware divide; Zap wraps
    // it to INT64_MIN (D-0019, #742).
    if a == i64::MIN && b == -1 {
        return Ok(i64::MIN);
    }
    let q = a / b;
    let r = a % b;
    // A nonzero remainder whose sign differs from the divisor rounds the
    // truncating quotient down by one (toward negative infinity).
    if r != 0 && ((r < 0) != (b < 0)) {
        Ok(q - 1)
    } else {
        Ok(q)
    }
}

/// `a % b` — floor-sign modulo (result takes the sign of the divisor),
/// mirroring `zap_imod`.
pub fn int_mod(a: i64, b: i64, span: Span) -> Result<i64, CtError> {
    if b == 0 {
        return Err(CtError::value(
            "integer division or modulo by zero at comptime",
            span,
        ));
    }
    // `a % -1 == 0` for every `a`; special-cased so INT64_MIN % -1 does not trap.
    if b == -1 {
        return Ok(0);
    }
    let r = a % b;
    if r != 0 && ((r < 0) != (b < 0)) {
        Ok(r + b)
    } else {
        Ok(r)
    }
}

/// `a << count`, mirroring `zap_ishl`.
pub fn int_shl(a: i64, count: i64, span: Span) -> Result<i64, CtError> {
    if count < 0 {
        return Err(CtError::value("negative shift count at comptime", span));
    }
    if count >= 64 {
        return Ok(0);
    }
    Ok(((a as u64) << (count as u64)) as i64)
}

/// `a >> count` — arithmetic right shift, mirroring `zap_ishr`.
pub fn int_shr(a: i64, count: i64, span: Span) -> Result<i64, CtError> {
    if count < 0 {
        return Err(CtError::value("negative shift count at comptime", span));
    }
    if count >= 64 {
        return Ok(if a < 0 { -1 } else { 0 });
    }
    Ok(a >> count)
}

// ---------------------------------------------------------------------------
// comparison (==, !=, <, <=, >, >=)
// ---------------------------------------------------------------------------

/// Structural `==` over the comparable B1 value types. Scalars and `str`/`char`
/// compare by value; `list`/`tuple` compare element-wise (recursively).
/// Comparing two different value kinds, or comparing reflection records, is a
/// type fault the evaluator never reaches in well-typed code.
pub fn value_eq(a: &Value, b: &Value, span: Span) -> Result<bool, CtError> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(x == y),
        (Value::Bool(x), Value::Bool(y)) => Ok(x == y),
        (Value::Char(x), Value::Char(y)) => Ok(x == y),
        (Value::Str(x), Value::Str(y)) => Ok(x == y),
        (Value::Kind(x), Value::Kind(y)) => Ok(x == y),
        (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => {
            if x.len() != y.len() {
                return Ok(false);
            }
            for (ex, ey) in x.iter().zip(y.iter()) {
                if !value_eq(ex, ey, span)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Err(CtError::value(
            format!(
                "cannot compare `{}` and `{}` at comptime",
                a.type_name(),
                b.type_name()
            ),
            span,
        )),
    }
}

/// Ordering (`<`, `<=`, `>`, `>=`) over the ordered B1 scalars: `int64`, `char`
/// (by codepoint), and `str` (by codepoint, which for valid UTF-8 equals byte
/// order). Returns the [`std::cmp::Ordering`]. Ordering any other value kind,
/// or mixed kinds, is a type fault.
pub fn value_cmp(a: &Value, b: &Value, span: Span) -> Result<std::cmp::Ordering, CtError> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(x.cmp(y)),
        (Value::Char(x), Value::Char(y)) => Ok(x.cmp(y)),
        (Value::Str(x), Value::Str(y)) => Ok(x.as_ref().cmp(y.as_ref())),
        _ => Err(CtError::value(
            format!(
                "cannot order `{}` and `{}` at comptime",
                a.type_name(),
                b.type_name()
            ),
            span,
        )),
    }
}

// ---------------------------------------------------------------------------
// string / char / aggregate operations
// ---------------------------------------------------------------------------

/// ASCII-only upper-case fold, mirroring `zap_str_upper` (bytes `a`–`z` → `A`–`Z`,
/// every other byte unchanged — no Unicode case mapping).
pub fn str_upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

/// ASCII-only lower-case fold, mirroring `zap_str_lower`.
pub fn str_lower(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect()
}

/// Codepoint index `s[i]` → `char` (negative indices count from the end),
/// mirroring `zap_str_index`: out of range is an `IndexError`-equivalent fault.
pub fn str_index(s: &str, i: i64, span: Span) -> Result<char, CtError> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len() as i64;
    let idx = if i < 0 { i + n } else { i };
    if idx < 0 || idx >= n {
        return Err(CtError::value(
            "string index out of range at comptime",
            span,
        ));
    }
    Ok(chars[idx as usize])
}

/// Element index `xs[i]` (negative counts from the end); out of range faults.
pub fn seq_index(xs: &[Value], i: i64, span: Span, what: &str) -> Result<Value, CtError> {
    let n = xs.len() as i64;
    let idx = if i < 0 { i + n } else { i };
    if idx < 0 || idx >= n {
        return Err(CtError::value(
            format!("{what} index out of range at comptime"),
            span,
        ));
    }
    Ok(xs[idx as usize].clone())
}

/// Normalize a positive-step slice bound to `[0, n]`, mirroring `zap_slice_pos`.
fn slice_pos(v: i64, n: i64) -> i64 {
    let mut v = v;
    if v < 0 {
        v += n;
    }
    if v < 0 {
        v = 0;
    }
    if v > n {
        v = n;
    }
    v
}

/// Normalize a negative-step slice bound to `[-1, n-1]`, mirroring `zap_slice_neg`.
fn slice_neg(v: i64, n: i64) -> i64 {
    let mut v = v;
    if v < 0 {
        v += n;
    }
    if v < -1 {
        v = -1;
    }
    if v > n - 1 {
        v = n - 1;
    }
    v
}

/// The sequence of source indices a `lo:hi:step` slice selects, Python
/// semantics (clamping bounds via [`slice_pos`]/[`slice_neg`]); a zero step is a
/// fault. Shared by string and list slicing so both match the runtime exactly.
pub fn slice_indices(
    n: i64,
    lo: Option<i64>,
    hi: Option<i64>,
    step: Option<i64>,
    span: Span,
) -> Result<Vec<usize>, CtError> {
    let st = step.unwrap_or(1);
    if st == 0 {
        return Err(CtError::value(
            "slice step cannot be zero at comptime",
            span,
        ));
    }
    let (start, stop) = if st > 0 {
        (
            lo.map(|v| slice_pos(v, n)).unwrap_or(0),
            hi.map(|v| slice_pos(v, n)).unwrap_or(n),
        )
    } else {
        (
            lo.map(|v| slice_neg(v, n)).unwrap_or(n - 1),
            hi.map(|v| slice_neg(v, n)).unwrap_or(-1),
        )
    };
    let mut out = Vec::new();
    let mut k = start;
    while if st > 0 { k < stop } else { k > stop } {
        out.push(k as usize);
        k += st;
    }
    Ok(out)
}

/// `chr(n)` / `char(n)` — validate `n` is a Unicode scalar value (not a
/// surrogate, in range), mirroring `zap_chr`.
pub fn int_to_char(n: i64, span: Span) -> Result<char, CtError> {
    if !(0..=0x10FFFF).contains(&n) || (0xD800..=0xDFFF).contains(&n) {
        return Err(CtError::value(
            "chr() argument is not a valid Unicode scalar value at comptime",
            span,
        ));
    }
    // Safe: the guard above excludes surrogates and out-of-range values.
    Ok(char::from_u32(n as u32).expect("validated Unicode scalar"))
}

/// `str(x)` — the textual form of a B1 scalar, mirroring the runtime:
/// decimal for `int64`, `True`/`False` for `bool` (Python-style, `zap_bool_to_str`),
/// the one-codepoint encoding for `char`, and identity for `str`.
pub fn value_to_str(v: &Value, span: Span) -> Result<String, CtError> {
    match v {
        Value::Int(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(if *b { "True" } else { "False" }.to_string()),
        Value::Char(c) => Ok(c.to_string()),
        Value::Str(s) => Ok(s.to_string()),
        _ => Err(CtError::not_available(
            format!("`str()` of a `{}` value", v.type_name()),
            span,
        )),
    }
}
