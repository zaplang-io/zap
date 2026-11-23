//! #1337: sema's value inference for a struct field read (`Expr::Attr` on a
//! value base) returned `Ty::Unknown` instead of the field's declared type, so
//! a type mismatch flowing through a field read was silently accepted and the
//! field-read span was never typed (a hole in the `TypeMap` the monomorphizer
//! consults, #1167). The `Expr::Attr` arm now consults `struct_info`/`fields`
//! and returns the named field's resolved type.
//!
//! These tests pin both directions: a mismatch through a field read is now a
//! clear error (ZAP4001 assignment, ZAP4021 argument), and the field-read span
//! carries the field's type in the `TypeMap`. Correct reads stay clean, a
//! generic struct's type-parameter-typed field stays `Unknown` (fail-closed —
//! the `Ty` model carries no type arguments), and an unknown attribute is
//! unchanged.

use zapc_sema::analyze_types;
use zapc_syntax::ast::{Expr, Item, Stmt};
use zapc_syntax::parser::parse_str;
use zapc_syntax::span::Span;

/// Error codes (as `ZAPnnnn` strings) sema reports for `src`.
fn codes(src: &str) -> Vec<String> {
    let (module, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (errs, _warns, _types) = analyze_types(&module);
    errs.iter().map(|e| format!("{:?}", e.code)).collect()
}

/// Assert `src` reports an error whose code renders as `code`.
fn assert_errs(src: &str, code: &str) {
    let got = codes(src);
    assert!(
        got.iter().any(|c| c.contains(code)),
        "expected {code}, got {got:?}"
    );
}

/// Assert `src` type-checks with no errors.
fn assert_clean(src: &str) {
    let got = codes(src);
    assert!(got.is_empty(), "expected clean, got {got:?}");
}

fn span_of(e: &Expr) -> Span {
    match e {
        Expr::Attr { span, .. }
        | Expr::Name(_, span)
        | Expr::Index { span, .. }
        | Expr::Call { span, .. } => *span,
        other => panic!("span_of: unhandled {other:?}"),
    }
}

/// Find `auto probe = <expr>` anywhere (free functions and struct methods) and
/// return the `Ty::display()` sema recorded for that value expression.
fn probe_type(src: &str) -> String {
    let (module, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");

    fn find(stmts: &[Stmt]) -> Option<Span> {
        for stmt in stmts {
            if let Stmt::Let { name, value, .. } = stmt {
                if name == "probe" {
                    return Some(span_of(value));
                }
            }
        }
        None
    }

    let mut probe_span = None;
    for item in &module.items {
        match item {
            Item::Function(f) => probe_span = probe_span.or_else(|| find(&f.body.stmts)),
            Item::Struct(s) => {
                for m in &s.methods {
                    probe_span = probe_span.or_else(|| find(&m.body.stmts));
                }
            }
            _ => {}
        }
    }
    let span = probe_span.expect("no `probe` binding found");

    let (errs, _warns, types) = analyze_types(&module);
    assert!(errs.is_empty(), "sema errors: {errs:?}");
    types
        .type_at(span)
        .unwrap_or_else(|| panic!("no type recorded for the probe expression"))
        .display()
}

fn assert_probe(src: &str, expected: &str) {
    assert_eq!(probe_type(src), expected, "probe field-read type mismatch");
}

// ---------------------------------------------------------------------------
// A mismatch through a field read is now reported.
// ---------------------------------------------------------------------------

#[test]
fn list_field_assigned_to_int_is_rejected() {
    // The issue's exact reproduction.
    assert_errs(
        "struct C:\n\
        \x20   int64 v\n\
        struct M:\n\
        \x20   list[C] cs\n\
        \x20   def f(self):\n\
        \x20       int64 x = self.cs\n\
        \x20       print(x)\n",
        "ZAP4001",
    );
}

#[test]
fn scalar_field_assigned_to_wrong_type_is_rejected() {
    assert_errs(
        "struct M:\n\
        \x20   int64 n\n\
        \x20   def f(self):\n\
        \x20       str s = self.n\n\
        \x20       print(s)\n",
        "ZAP4001",
    );
}

#[test]
fn chained_field_read_mismatch_is_rejected() {
    // A field read of a field read must also be typed (nested struct).
    assert_errs(
        "struct Inner:\n\
        \x20   int64 v\n\
        struct Outer:\n\
        \x20   Inner inner\n\
        \x20   def f(self):\n\
        \x20       str s = self.inner.v\n\
        \x20       print(s)\n",
        "ZAP4001",
    );
}

#[test]
fn field_read_as_mistyped_argument_is_rejected() {
    // A field read passed where the parameter type disagrees (ZAP4021).
    assert_errs(
        "struct M:\n\
        \x20   int64 n\n\
        def take(str s) -> int64:\n\
        \x20   return 1\n\
        def g(M m):\n\
        \x20   print(take(m.n))\n",
        "ZAP4021",
    );
}

#[test]
fn field_read_on_a_local_receiver_is_rejected() {
    // Not just `self.field` — a field read off any struct-typed value.
    assert_errs(
        "struct M:\n\
        \x20   int64 n\n\
        def g():\n\
        \x20   M m = M(5)\n\
        \x20   str s = m.n\n\
        \x20   print(s)\n",
        "ZAP4001",
    );
}

// ---------------------------------------------------------------------------
// Correct reads stay clean (regressions), and the loose cases stay loose.
// ---------------------------------------------------------------------------

#[test]
fn correct_field_read_is_clean() {
    assert_clean(
        "struct M:\n\
        \x20   int64 n\n\
        \x20   def f(self):\n\
        \x20       int64 x = self.n\n\
        \x20       print(x)\n",
    );
}

#[test]
fn correct_chained_and_container_field_reads_are_clean() {
    assert_clean(
        "struct Inner:\n\
        \x20   int64 v\n\
        struct Outer:\n\
        \x20   Inner inner\n\
        \x20   list[int64] xs\n\
        \x20   def f(self):\n\
        \x20       int64 a = self.inner.v\n\
        \x20       list[int64] b = self.xs\n\
        \x20       print(a)\n\
        \x20       print(b)\n",
    );
}

#[test]
fn local_binding_mismatch_still_rejected() {
    // The already-working local path must not regress.
    assert_errs(
        "def f():\n\
        \x20   list[int64] cs = [1]\n\
        \x20   int64 x = cs\n\
        \x20   print(x)\n",
        "ZAP4001",
    );
}

#[test]
fn generic_struct_param_field_is_fail_closed() {
    // A type-parameter-typed field resolves to `Unknown` (the `Ty` model carries
    // no type arguments), so a mismatch through it is NOT falsely rejected — the
    // conservative pre-#1337 behavior is preserved for generics.
    assert_clean(
        "struct Box[T]:\n\
        \x20   T item\n\
        def g(Box[int64] b):\n\
        \x20   str s = b.item\n\
        \x20   print(s)\n",
    );
}

#[test]
fn unknown_attribute_is_unchanged() {
    // Reading an attribute the struct does not declare stays loose (no new
    // "unknown field" diagnostic is introduced by this change).
    assert_clean(
        "struct M:\n\
        \x20   int64 n\n\
        \x20   def f(self):\n\
        \x20       auto x = self.missing\n\
        \x20       print(x)\n",
    );
}

// ---------------------------------------------------------------------------
// The field-read span is now typed in the `TypeMap` (what mono reads, #1167).
// ---------------------------------------------------------------------------

#[test]
fn scalar_field_read_is_typed_int64() {
    assert_probe(
        "struct M:\n\
        \x20   int64 n\n\
        \x20   def f(self):\n\
        \x20       auto probe = self.n\n\
        \x20       print(probe)\n",
        "int64",
    );
}

#[test]
fn list_field_read_is_typed_list() {
    assert_probe(
        "struct M:\n\
        \x20   list[int64] xs\n\
        \x20   def f(self):\n\
        \x20       auto probe = self.xs\n\
        \x20       print(probe)\n",
        "list[int64]",
    );
}

#[test]
fn struct_field_read_is_typed_nominal() {
    assert_probe(
        "struct Inner:\n\
        \x20   int64 v\n\
        struct Outer:\n\
        \x20   Inner inner\n\
        \x20   def f(self):\n\
        \x20       auto probe = self.inner\n\
        \x20       print(probe.v)\n",
        "Inner",
    );
}

#[test]
fn chained_field_read_is_typed() {
    assert_probe(
        "struct Inner:\n\
        \x20   int64 v\n\
        struct Outer:\n\
        \x20   Inner inner\n\
        \x20   def f(self):\n\
        \x20       auto probe = self.inner.v\n\
        \x20       print(probe)\n",
        "int64",
    );
}
