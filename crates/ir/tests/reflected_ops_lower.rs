//! Reflected binary-operator lowering (D-0047 / #1180, epic #1189).
//!
//! The sema slice (#1186, merged in #1192) resolves `a OP b` against the RIGHT
//! operand's reflected dunder (`__radd__` … `__rmatmul__`) as a fallback when
//! the left operand cannot handle the operator itself. This slice (#1187) makes
//! AST→ZIR lowering emit the matching call: `a + b` -> `b.__radd__(a)` with the
//! right operand as `self`. Precedence mirrors sema — the left/built-in path
//! wins; reflected is used only where the expression would otherwise fail.
//!
//! These are host-only checks (no LLVM): `lower_module` runs mono + lowering, so
//! a missing-dunder failure or a desugaring gap surfaces here. The runtime
//! value round-trips (that `dt + rd` actually calls into the package code) are
//! pinned by CI goldens.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting the whole frontend (mono included) accepts it.
fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
}

/// Parse + lower `src`, asserting lowering fails with a message containing
/// `needle` (lex/parse must be clean so the failure is the lowering one).
fn lower_err_contains(src: &str, needle: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(!errs.is_empty(), "expected a lowering error, got none");
    let joined = errs
        .iter()
        .map(|e| format!("{e:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains(needle),
        "expected a lowering error containing {needle:?}, got:\n{joined}"
    );
}

// ----- the motivating shape: right operand owns the operator -----

#[test]
fn reflected_add_and_sub_lower_via_right_operand() {
    // `DT` (left) defines NO forward `__add__`/`__sub__`; `Rel` (right) supplies
    // the reflected handlers. Both expressions must lower to the reflected call.
    lower_ok(
        "value struct DT:\n    int64 month\n\n\
         struct Rel:\n    int64 months\n\n    \
         def __radd__(self, DT dt) -> DT:\n        return dt\n\n    \
         def __rsub__(self, DT dt) -> DT:\n        return dt\n\n\
         def main():\n    auto dt = DT(month=1)\n    auto rd = Rel(months=1)\n    \
         auto a = dt + rd\n    auto b = dt - rd\n    print(a.month + b.month)\n",
    );
}

#[test]
fn reflected_scalar_left_mul_lowers() {
    // Built-in left (`f64`) with no rule for the struct; the struct's `__rmul__`
    // must be reached via the built-in-left reflected path.
    lower_ok(
        "value struct Vec2:\n    f64 x\n\n    \
         def __rmul__(self, f64 k) -> Vec2:\n        return Vec2(x=self.x * k)\n\n\
         def main():\n    Vec2 v = Vec2(x=2.0)\n    Vec2 w = 3.0 * v\n    print(str(w.x))\n",
    );
}

#[test]
fn reflected_full_operator_set_lowers() {
    // Every binary operator with a reflected form, scalar-left + struct-right,
    // lowers through the reflected dunder.
    for (sym, rdunder) in [
        ("+", "__radd__"),
        ("-", "__rsub__"),
        ("*", "__rmul__"),
        ("/", "__rtruediv__"),
        ("//", "__rfloordiv__"),
        ("%", "__rmod__"),
        ("**", "__rpow__"),
        ("&", "__rand__"),
        ("|", "__ror__"),
        ("^", "__rxor__"),
        ("<<", "__rlshift__"),
        (">>", "__rrshift__"),
        ("@", "__rmatmul__"),
    ] {
        let src = format!(
            "struct W:\n    int64 n\n\n    \
             def {rdunder}(self, int64 k) -> W:\n        return W(n=k)\n\n\
             def main():\n    W r = 3 {sym} W(n=0)\n    print(r.n)\n"
        );
        lower_ok(&src);
    }
}

// ----- precedence: the left path wins -----

#[test]
fn left_forward_dunder_lowers_even_when_right_has_reflected() {
    // Both a left `__add__` and a right `__radd__` exist; lowering must take the
    // left path (it does not error, and the whole program lowers).
    lower_ok(
        "struct L:\n    int64 n\n\n    \
         def __add__(self, R other) -> L:\n        return L(n=self.n)\n\n\
         struct R:\n    int64 m\n\n    \
         def __radd__(self, L other) -> R:\n        return R(m=self.m)\n\n\
         def main():\n    auto a = L(n=1)\n    auto b = R(m=2)\n    auto c = a + b\n    print(c.n)\n",
    );
}

// ----- parameter-aware fall-through (#1200) -----

#[test]
fn left_forward_rejects_rhs_lowers_via_reflected() {
    // #1200: the left `V` defines `__add__(self, V)`, but the RHS is an unrelated
    // `Rd`. The forward parameter (`V`) does not accept an `Rd`, so lowering must
    // fall through to `Rd.__radd__(self, V) -> str` rather than forming a forward
    // call that would reinterpret the `Rd` pointer as a `V`. (The ir crate runs
    // without sema, so this exercises lowering's own parameter-aware selection.)
    lower_ok(
        "struct V:\n    int64 x\n\n    \
         def __add__(self, V other) -> V:\n        return V(x=self.x)\n\n\
         struct Rd:\n    int64 n\n\n    \
         def __radd__(self, V v) -> str:\n        return \"x\"\n\n\
         def main():\n    auto a = V(x=1)\n    str s = a + Rd(n=2)\n    print(s)\n",
    );
}

#[test]
fn generic_left_forward_rejects_mono_rhs_lowers_via_reflected() {
    // The crux case #1200 exists for: a GENERIC left whose forward parameter is a
    // type variable. Sema erases `T` to `Unknown` (permissive) and keeps the
    // forward commit; the real decision is made here on the monomorphized
    // signature. `Box[int64].__add__(self, int64)` is instantiated by `b + 5`
    // (forward, accepted). `b + Delta(..)` has a monomorphized forward parameter
    // of `int64`, which cannot accept a `Delta` pointer, so it must reroute to
    // `Delta.__radd__(self, Box[int64])`.
    lower_ok(
        "struct Box[T]:\n    T val\n\n    \
         def __add__(self, T other) -> Box[T]:\n        return Box[T](val=other)\n\n\
         struct Delta:\n    int64 k\n\n    \
         def __radd__(self, Box[int64] b) -> Delta:\n        return self\n\n\
         def main():\n    auto b = Box[int64](val=1)\n    \
         auto hit = b + 5\n    auto x = b + Delta(k=2)\n    print(hit.val + x.k)\n",
    );
}

#[test]
fn left_forward_rejects_rhs_no_reflected_is_clean_error() {
    // #1200: the left `V` defines `__add__(self, V)` but the RHS `Rd` is neither
    // a `V` nor equipped with a matching `__radd__`. Lowering must emit a clean
    // mismatch error (naming the forward dunder), NOT silently reinterpret the
    // `Rd` pointer as a `V` through `coerce`.
    lower_err_contains(
        "struct V:\n    int64 x\n\n    \
         def __add__(self, V other) -> V:\n        return V(x=self.x)\n\n\
         struct Rd:\n    int64 n\n\n\
         def main():\n    auto a = V(x=1)\n    auto bad = a + Rd(n=2)\n    print(bad.x)\n",
        "__add__",
    );
}

// ----- the error path is preserved -----

#[test]
fn neither_forward_nor_reflected_is_missing_dunder_error() {
    // A struct left with no forward dunder and a right struct WITHOUT the
    // reflected dunder is still the original clean "missing dunder" error naming
    // the forward dunder (sema would report ZAP4071 first; the ir crate is
    // exercised without sema).
    lower_err_contains(
        "value struct DT:\n    int64 month\n\n\
         struct Rel:\n    int64 months\n\n\
         def main():\n    auto dt = DT(month=1)\n    auto c = dt + Rel(months=1)\n    print(c.month)\n",
        "__add__",
    );
}

#[test]
fn comparison_needs_rcmp_not_other_reflected_dunder() {
    // Comparison reflects only through `__rcmp__`/`__req__` (#1198): a right
    // operand that defines an arithmetic reflected dunder (`__radd__`) but no
    // `__rcmp__` does not satisfy `<`, which still errors naming `__cmp__`.
    lower_err_contains(
        "value struct DT:\n    int64 month\n\n\
         struct Rel:\n    int64 months\n\n    \
         def __radd__(self, DT dt) -> DT:\n        return dt\n\n\
         def main():\n    auto dt = DT(month=1)\n    if dt < Rel(months=1):\n        print(1)\n",
        "__cmp__",
    );
}

// ----- reflected comparison (#1198) -----

#[test]
fn reflected_comparison_lowers_via_right_operand() {
    // Left `DT` has no `__cmp__`/`__eq__`; right `Rel` supplies `__rcmp__` /
    // `__req__`. All relationals and equality lower to the reflected call.
    lower_ok(
        "value struct DT:\n    int64 month\n\n\
         struct Rel:\n    int64 months\n\n    \
         def __rcmp__(self, DT dt) -> int64:\n        return 0\n\n    \
         def __req__(self, DT dt) -> bool:\n        return True\n\n\
         def main():\n    auto dt = DT(month=1)\n    auto rd = Rel(months=1)\n    \
         if dt < rd:\n        print(1)\n    if dt >= rd:\n        print(2)\n    \
         if dt == rd:\n        print(3)\n    if dt != rd:\n        print(4)\n",
    );
}

#[test]
fn reflected_comparison_builtin_left_lowers() {
    // Built-in left (`int64`) with a struct right defining the reflected
    // comparison dunders: `3 < v` / `3 == v`.
    lower_ok(
        "struct Money:\n    int64 cents\n\n    \
         def __rcmp__(self, int64 n) -> int64:\n        return 0\n\n    \
         def __req__(self, int64 n) -> bool:\n        return False\n\n\
         def main():\n    auto m = Money(cents=100)\n    if 3 < m:\n        print(1)\n    \
         if 3 == m:\n        print(2)\n",
    );
}
