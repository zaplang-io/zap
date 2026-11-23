//! #1309: an `Optional[T]` (or any generic instance) returned by an interface
//! method must keep its instantiated struct type through lowering, so a method
//! call on that result (`reader.end_time().is_some()`) lowers.
//!
//! Before the fix, monomorphization emitted the interface declaration verbatim:
//! the method's `Optional[int64]` return annotation was never renamed to the
//! mangled concrete instance (`Optional$int64`) nor requested, so IR lowering
//! resolved the un-mangled `Optional[…]` head with its type arguments ignored.
//! The interface call's result therefore lost its `Optional` struct type and the
//! chained `.is_some()` failed to lower with `ZAP6001` ("method call on a
//! non-struct value"). These tests run the real lowering pass (which prepends
//! the prelude and monomorphizes), host-only — no LLVM — so they guard the fix
//! wherever `cargo test` runs. End-to-end Some/None behavior and the
//! cross-module form are covered by the `iface_optional_return` golden fixture
//! and the `iface_optional_cross_module` golden project.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Lower `src`, asserting it parses and lowers with no errors.
fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_zir, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "expected clean lowering, got: {errs:?}");
}

#[test]
fn optional_return_method_call_as_expression() {
    // The issue's exact reproduction: `.is_some()` directly on the call result.
    lower_ok(
        "interface Reader:\n\
         \x20   def end_time(self) -> Optional[int64]\n\
         def finished(Reader reader) -> bool:\n\
         \x20   return reader.end_time().is_some()\n",
    );
}

#[test]
fn optional_return_method_call_via_local_binding() {
    // The issue states the same error occurs through an `auto` local; the result
    // type must survive the binding so the method resolves off the local too.
    lower_ok(
        "interface Reader:\n\
         \x20   def end_time(self) -> Optional[int64]\n\
         def finished(Reader reader) -> bool:\n\
         \x20   auto t = reader.end_time()\n\
         \x20   return t.is_some()\n",
    );
}

#[test]
fn optional_return_unwrap_and_is_none() {
    // Every `Optional` method must be reachable off the interface-call result,
    // not just `is_some`: `is_none` and `unwrap` lower too.
    lower_ok(
        "interface Reader:\n\
         \x20   def end_time(self) -> Optional[int64]\n\
         def still_running(Reader reader) -> bool:\n\
         \x20   return reader.end_time().is_none()\n\
         def stamp(Reader reader) -> int64:\n\
         \x20   return reader.end_time().unwrap()\n",
    );
}

#[test]
fn optional_return_with_struct_conformer_some_and_none() {
    // A full program: a struct conforms to the interface, returning both a
    // `Some` and a `None`, and the result is consumed behind the interface type.
    // This is what the otel span/processor shape needs (#1309 downstream).
    lower_ok(
        "interface Reader:\n\
         \x20   def end_time(self) -> Optional[int64]\n\
         struct EndedSpan:\n\
         \x20   int64 t\n\
         \x20   def end_time(self) -> Optional[int64]:\n\
         \x20       return Optional.Some(self.t)\n\
         struct LiveSpan:\n\
         \x20   def end_time(self) -> Optional[int64]:\n\
         \x20       return Optional.None\n\
         def finished(Reader reader) -> bool:\n\
         \x20   return reader.end_time().is_some()\n\
         def main():\n\
         \x20   Reader a = EndedSpan(t=5)\n\
         \x20   Reader b = LiveSpan()\n\
         \x20   print(finished(a))\n\
         \x20   print(finished(b))\n",
    );
}

#[test]
fn optional_of_string_return() {
    // A different payload type (`Optional[str]`) is mangled and resolved just as
    // `Optional[int64]` is — the fix is not special-cased to one payload.
    lower_ok(
        "interface Labeled:\n\
         \x20   def label(self) -> Optional[str]\n\
         def has_label(Labeled x) -> bool:\n\
         \x20   return x.label().is_some()\n",
    );
}

#[test]
fn nested_optional_of_list_return() {
    // A compound generic return (`Optional[list[int64]]`) mangles through its
    // nested argument and still resolves to a struct the methods dispatch on.
    lower_ok(
        "interface Rows:\n\
         \x20   def head(self) -> Optional[list[int64]]\n\
         def has_rows(Rows r) -> bool:\n\
         \x20   return r.head().is_some()\n",
    );
}

#[test]
fn optional_return_through_refined_supertype() {
    // The return type survives even when the method is inherited through a
    // refined supertype (#830/D-0139): `Timed` refines `Reader`, and a `Timed`
    // value still calls the supertype's `Optional`-returning method.
    lower_ok(
        "interface Reader:\n\
         \x20   def end_time(self) -> Optional[int64]\n\
         interface Timed: Reader\n\
         \x20   def started(self) -> bool\n\
         def finished(Timed t) -> bool:\n\
         \x20   return t.end_time().is_some()\n",
    );
}

#[test]
fn plain_non_generic_interface_return_still_lowers() {
    // Regression guard: an interface method returning a non-generic type is
    // unaffected by the signature rewrite and still lowers.
    lower_ok(
        "interface Named:\n\
         \x20   def name(self) -> int64\n\
         def label(Named n) -> int64:\n\
         \x20   return n.name()\n",
    );
}
