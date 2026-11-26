//! Binding a generic *method's* return type into the call result (#1643) — the
//! method-side counterpart of #1623, which did this for free functions.
//!
//! #1623 substituted a generic free function's bound type parameters into its
//! return type; a generic *method's* return type was left loose (`sig.ret`) to
//! preserve the method-chain operator typing the instance-method path relies on,
//! so a mismatched binding target on a generic method call was not flagged. The
//! struct instance-method path now uses the substituted return type that
//! `check_generic_arg_consistency` already computes — but ONLY when the method is
//! generic and the binding produced a concrete type, so a non-generic method and
//! an unbound generic return stay loose and operator-chain dispatch is preserved.
//!
//! Host-only sema checks (no LLVM). Additive: a matching target, numeric
//! widening, a non-generic method result, and an operator-overloading method
//! chain are unchanged.

use zapc_sema::check_str;

#[track_caller]
fn sema_errors(src: &str) -> Vec<(String, String)> {
    let (terrs, perrs, lerrs) = check_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    terrs
        .into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect()
}

#[track_caller]
fn assert_ok(src: &str) {
    let errs = sema_errors(src);
    assert!(errs.is_empty(), "expected no sema errors, got: {errs:?}");
}

#[track_caller]
fn assert_rejected(src: &str) {
    let errs = sema_errors(src);
    assert!(!errs.is_empty(), "expected some sema error, got none");
}

// ---- the issue repro ------------------------------------------------------

#[test]
fn generic_method_return_rejects_mismatched_target() {
    // `make[T](self, Type[T] kind, T default) -> T` binds `T = int64`, so the
    // result is `int64`; assigning it to `str` must be rejected (previously the
    // method return erased to `Unknown` and the assignment was accepted).
    assert_rejected(
        "struct Opt:\n    def make[T](self, Type[T] kind, T default) -> T:\n        return default\n\
         def main():\n    Opt o = Opt()\n    str s = o.make(int64, 1)\n    print(s)\n",
    );
}

#[test]
fn generic_method_return_binds_matching_target() {
    assert_ok(
        "struct Opt:\n    def make[T](self, Type[T] kind, T default) -> T:\n        return default\n\
         def main():\n    Opt o = Opt()\n    int64 n = o.make(int64, 1)\n    print(n)\n",
    );
}

// ---- bare `T` argument binds the result ----------------------------------

#[test]
fn generic_method_return_from_bare_t_argument() {
    assert_ok(
        "struct Box:\n    def ident[T](self, T x) -> T:\n        return x\n\
         def main():\n    Box b = Box()\n    int64 n = b.ident(5)\n    print(n)\n",
    );
    assert_rejected(
        "struct Box:\n    def ident[T](self, T x) -> T:\n        return x\n\
         def main():\n    Box b = Box()\n    str s = b.ident(5)\n    print(s)\n",
    );
}

#[test]
fn generic_method_return_numeric_widening_ok() {
    // Binding `T = int32` from the argument; assigning to the wider `int64` is a
    // legal widening, not a mismatch.
    assert_ok(
        "struct Box:\n    def ident[T](self, T x) -> T:\n        return x\n\
         def main():\n    Box b = Box()\n    int32 x = 7\n    int64 n = b.ident(x)\n    print(n)\n",
    );
}

// (An unbound generic *method* return — a type parameter that appears only in the
// return and is bound by no argument — is not constructible as valid source: the
// body would have no value of that type to return. The substitution still maps
// such a parameter to `Unknown`, keeping the result loose, mirroring #1623's
// free-function behaviour; `generic_method_returning_self_struct_preserved` below
// covers the related "return type is concrete, not a bound parameter" path.)

// ---- non-generic method result is unchanged ------------------------------

#[test]
fn non_generic_method_result_unchanged() {
    assert_ok(
        "struct S:\n    def plain(self, int64 x) -> int64:\n        return x\n\
         def main():\n    S s = S()\n    int64 n = s.plain(3)\n    print(n)\n",
    );
    // A non-generic method returning the wrong concrete type for the target is
    // still rejected by the ordinary assignment check (the result is its declared
    // concrete type, not erased).
    assert_rejected(
        "struct S:\n    def plain(self, int64 x) -> int64:\n        return x\n\
         def main():\n    S s = S()\n    str t = s.plain(3)\n    print(t)\n",
    );
}

// ---- method-chain operator dispatch is preserved -------------------------

#[test]
fn method_chain_operator_dispatch_preserved() {
    // A non-generic fluent builder: each method returns a concrete `Cond` struct,
    // and `&` dispatches on it via `__and__`. The returned struct type must be
    // kept (not erased) for the operator to resolve — the return-binding change
    // must not disturb this.
    assert_ok(
        "struct Cond:\n    int64 tag\n    def __and__(self, Cond other) -> Cond:\n        return self\n\
         struct Builder:\n    def eq(self, int64 v) -> Cond:\n        return Cond(v)\n    \
         def gt(self, int64 v) -> Cond:\n        return Cond(v)\n\
         def main():\n    Builder b = Builder()\n    Cond c = b.eq(1) & b.gt(2)\n    print(c.tag)\n",
    );
}

#[test]
fn generic_method_returning_self_struct_preserved() {
    // A *generic* method that returns a concrete struct (no type parameter in the
    // return) keeps that struct type, so a chained operator still dispatches.
    assert_ok(
        "struct Cond:\n    int64 tag\n    def __and__(self, Cond other) -> Cond:\n        return self\n\
         struct Builder:\n    def eqv[T](self, Type[T] kind, T v) -> Cond:\n        return Cond(1)\n\
         def main():\n    Builder b = Builder()\n    Cond c = b.eqv(int64, 1) & b.eqv(int64, 2)\n    print(c.tag)\n",
    );
}
