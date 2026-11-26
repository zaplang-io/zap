//! Per-argument type-checking for struct *instance*-method calls (#1653) — the
//! struct-side parity counterpart of #1642 (which added it for enum methods). The
//! struct instance-method call path previously ran only the generic
//! type-parameter consistency pass (#1622) and otherwise inferred arguments
//! loosely, so a plain (non-generic) argument mismatch on a struct method was
//! silently accepted — while the equivalent enum method call rejects it (#1642).
//!
//! The struct call path now also checks each positional argument `coercible`
//! against the declared parameter (the implicit `self` is `params[0]`, so a
//! positional argument `i` maps to `params[i + 1]`) and each keyword argument
//! against its named parameter, reporting `ZAP4021` / `ZAP4022`. Arity stays
//! loose and the method's return type is unchanged (generic return binding is
//! #1643), so the method-chain operator typing this path drives `&`/`|`/`~`
//! dispatch from is preserved.
//!
//! Host-only sema checks (no LLVM). Additive: a consistent call, numeric
//! widening, and an operator-overloading method chain are unchanged.

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
fn assert_code(src: &str, code: &str) {
    let errs = sema_errors(src);
    assert!(
        errs.iter().any(|(c, _)| c == code),
        "expected a {code} error, got: {errs:?}",
    );
}

#[track_caller]
fn assert_rejected(src: &str) {
    let errs = sema_errors(src);
    assert!(!errs.is_empty(), "expected some sema error, got none");
}

// ---- plain (non-generic) argument type checking --------------------------

#[test]
fn struct_plain_method_rejects_arg_type_mismatch() {
    assert_code(
        "struct S:\n    def plain(self, int64 n) -> int64:\n        return n\n\
         def main():\n    S s = S()\n    auto w = s.plain(\"not an int\")\n    print(w)\n",
        "ZAP4021",
    );
}

#[test]
fn struct_plain_method_accepts_matching_arg() {
    assert_ok(
        "struct S:\n    def plain(self, int64 n) -> int64:\n        return n\n\
         def main():\n    S s = S()\n    auto w = s.plain(7)\n    print(w)\n",
    );
}

#[test]
fn struct_plain_method_multi_arg_mix() {
    // The positional-to-parameter mapping skips `self`: a good first argument and
    // a bad second one is still rejected.
    assert_code(
        "struct S:\n    def pair(self, int64 n, str s) -> int64:\n        return n\n\
         def main():\n    S s = S()\n    auto w = s.pair(1, 2)\n    print(w)\n",
        "ZAP4021",
    );
    assert_ok(
        "struct S:\n    def pair(self, int64 n, str s) -> int64:\n        return n\n\
         def main():\n    S s = S()\n    auto w = s.pair(1, \"ok\")\n    print(w)\n",
    );
}

#[test]
fn struct_plain_method_numeric_widening_ok() {
    assert_ok(
        "struct S:\n    def plain(self, int64 n) -> int64:\n        return n\n\
         def main():\n    S s = S()\n    int32 x = 3\n    auto w = s.plain(x)\n    print(w)\n",
    );
}

#[test]
fn struct_plain_method_keyword_arg_mismatch_rejected() {
    assert_rejected(
        "struct S:\n    def plain(self, int64 n) -> int64:\n        return n\n\
         def main():\n    S s = S()\n    auto w = s.plain(n=\"nope\")\n    print(w)\n",
    );
}

#[test]
fn value_struct_plain_method_rejects_mismatch() {
    assert_code(
        "value struct V:\n    int64 x\n    def plain(self, int64 n) -> int64:\n        return n\n\
         def main():\n    V v = V(5)\n    auto w = v.plain(\"nope\")\n    print(w)\n",
        "ZAP4021",
    );
}

// ---- generic method consistency unchanged (#1622) ------------------------

#[test]
fn struct_generic_method_still_rejects_inconsistent_args() {
    assert_code(
        "struct Util:\n    def take[T](self, T x, T y) -> T:\n        return x\n\
         def main():\n    Util u = Util()\n    auto z = u.take(1, \"hello\")\n    print(z)\n",
        "ZAP4021",
    );
}

#[test]
fn struct_generic_method_accepts_consistent_args() {
    assert_ok(
        "struct Util:\n    def take[T](self, T x, T y) -> T:\n        return x\n\
         def main():\n    Util u = Util()\n    auto a = u.take(1, 2)\n    auto b = u.take(\"a\", \"b\")\n    print(a)\n",
    );
}

// ---- operator-overloading method chain is preserved ----------------------

#[test]
fn method_chain_operator_dispatch_still_typechecks() {
    // A fluent condition builder (operators #52): each method returns a `Cond`
    // struct, and `&` dispatches on the returned struct via its `__and__` dunder.
    // Per-argument type-checking must not disturb this: a correctly-typed chain
    // still compiles, and the operator result stays the struct.
    assert_ok(
        "struct Cond:\n    int64 tag\n    def __and__(self, Cond other) -> Cond:\n        return self\n\
         struct Builder:\n    def eq(self, int64 v) -> Cond:\n        return Cond(v)\n    \
         def gt(self, int64 v) -> Cond:\n        return Cond(v)\n\
         def main():\n    Builder b = Builder()\n    Cond c = b.eq(1) & b.gt(2)\n    print(c.tag)\n",
    );
}

#[test]
fn method_chain_with_bad_arg_still_rejected() {
    // The same chain with a wrongly-typed argument to a chained method is now
    // caught (previously the argument went unchecked).
    assert_code(
        "struct Cond:\n    int64 tag\n    def __and__(self, Cond other) -> Cond:\n        return self\n\
         struct Builder:\n    def eq(self, int64 v) -> Cond:\n        return Cond(v)\n    \
         def gt(self, int64 v) -> Cond:\n        return Cond(v)\n\
         def main():\n    Builder b = Builder()\n    Cond c = b.eq(\"x\") & b.gt(2)\n    print(c.tag)\n",
        "ZAP4021",
    );
}
