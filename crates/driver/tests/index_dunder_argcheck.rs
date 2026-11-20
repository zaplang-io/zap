//! The index-overload dunders `__getitem__` / `__setitem__` (D-0047) must
//! type-check their index (and, for `__setitem__`, value) argument against the
//! dunder's declared parameter type (#1103).
//!
//! Before #1103 the `a[i]` -> `a.__getitem__(i)` / `a[i] = v` ->
//! `a.__setitem__(i, v)` rewrite formed the call without checking that the
//! supplied argument's representation matches the parameter: a mismatched index
//! (e.g. an `i64` where a `str`/pointer key is expected) was accepted and lowered
//! to a call that fed the wrong word kind into the method (an `iconst` into a
//! pointer slot — a latent type confusion).
//!
//! Sema cannot catch this: a generic struct's instantiation type argument
//! (`Box[str]`) is erased to the bare `Named("Box")`, and the type variable `K`
//! resolves to the universally assignable `Unknown` (see the #465 note — sema has
//! no model of a monomorphized generic method's parameter types). The real,
//! monomorphized signature (`Box$str__getitem__ : (struct, ptr) -> i64`) exists
//! only in AST->ZIR lowering, so — exactly like the #465 fn-value coercion check
//! — the soundness backstop lives there. A `LowerError` is a real compile error
//! (`zap ir` / `zap build` exit non-zero) reported before codegen.
//!
//! These assert the stable `ZAP4021` code (the same code a normal argument/
//! parameter mismatch produces) exercised through the real front-end + lowering
//! pipeline. They run on the host (no LLVM).

use zapc_ir::LowerError;

/// Run the front-end (parse + sema) then AST->ZIR lowering, returning the
/// lowering errors. Asserts the program lexes, parses, and type-checks cleanly —
/// sema accepts the misuse (it has erased the generic instantiation), so the
/// backstop must be the lowering stage.
fn lower_errors(src: &str) -> Vec<LowerError> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    let serrs = zapc_sema::check(&module);
    assert!(
        serrs.is_empty(),
        "sema is expected to accept the index-dunder misuse (it erases the \
         generic instantiation and has no concrete parameter type to check); \
         got {serrs:?}"
    );
    zapc_ir::lower_module(&module).1
}

/// Assert `src` lowers to exactly one error with `code` and a message containing
/// every expected fragment.
#[track_caller]
fn assert_rejected(src: &str, code: &str, fragments: &[&str]) {
    let errs = lower_errors(src);
    assert_eq!(
        errs.len(),
        1,
        "expected exactly one lowering error, got {errs:?}"
    );
    let e = &errs[0];
    assert_eq!(
        e.code.to_string(),
        code,
        "wrong code for message {:?}",
        e.message
    );
    for frag in fragments {
        assert!(
            e.message.contains(frag),
            "message {:?} is missing fragment {frag:?}",
            e.message
        );
    }
}

#[track_caller]
fn assert_accepted(src: &str) {
    let errs = lower_errors(src);
    assert!(errs.is_empty(), "expected clean lowering, got {errs:?}");
}

/// A generic `Box[K]` with `__getitem__`/`__setitem__` over a `dict[K, int64]`.
/// `{key}` is spliced into `main` as the sole index expression.
fn getitem_prog(inst: &str, key: &str) -> String {
    format!(
        "struct Box[K]:\n\
        \x20   dict[K, int64] m\n\
        \x20   def __getitem__(self, K key) -> int64:\n\
        \x20       return self.m.get(key, 0)\n\n\
         def main():\n\
        \x20   dict[{inst}, int64] d = {{}}\n\
        \x20   Box[{inst}] b = Box(m=d)\n\
        \x20   print(b[{key}])\n"
    )
}

// ---------------------------------------------------------------------------
// #1103 repro: the index argument's type is not checked (generic struct).
// ---------------------------------------------------------------------------

#[test]
fn getitem_wrong_index_literal_rejected() {
    // The issue's exact repro: `b[5]` on a `Box[str]` — the int literal `5`
    // (a scalar `i64`) fed into the `str` (pointer) key parameter.
    assert_rejected(&getitem_prog("str", "5"), "ZAP4021", &["__getitem__"]);
}

#[test]
fn getitem_wrong_index_typed_binding_rejected() {
    // Not just untyped-literal coercion: a typed `int64 bad = 5` is rejected
    // too (the issue is explicit about this).
    let src = "struct Box[K]:\n\
        \x20   dict[K, int64] m\n\
        \x20   def __getitem__(self, K key) -> int64:\n\
        \x20       return self.m.get(key, 0)\n\n\
         def main():\n\
        \x20   dict[str, int64] d = {}\n\
        \x20   Box[str] b = Box(m=d)\n\
        \x20   int64 bad = 5\n\
        \x20   print(b[bad])\n";
    assert_rejected(src, "ZAP4021", &["__getitem__"]);
}

#[test]
fn getitem_correct_index_accepted() {
    // The correctly-typed index (`str` into a `str` key) lowers cleanly — the
    // check must not over-reject the happy path.
    assert_accepted(&getitem_prog("str", "\"a\""));
}

#[test]
fn getitem_int_key_correct_accepted() {
    // A `Box[int64]` indexed by an int is correct and must still lower.
    assert_accepted(&getitem_prog("int64", "5"));
}

#[test]
fn getitem_int_key_wrong_str_rejected() {
    // The mirror image: a `Box[int64]` indexed by a `str` — a pointer where a
    // scalar `i64` key is expected.
    assert_rejected(&getitem_prog("int64", "\"a\""), "ZAP4021", &["__getitem__"]);
}

// ---------------------------------------------------------------------------
// Non-generic struct with a concrete-typed __getitem__.
// ---------------------------------------------------------------------------

#[test]
fn getitem_nongeneric_concrete_wrong_rejected() {
    let src = "struct SBox:\n\
        \x20   dict[str, int64] m\n\
        \x20   def __getitem__(self, str key) -> int64:\n\
        \x20       return self.m.get(key, 0)\n\n\
         def main():\n\
        \x20   dict[str, int64] d = {}\n\
        \x20   SBox b = SBox(m=d)\n\
        \x20   print(b[5])\n";
    assert_rejected(src, "ZAP4021", &["__getitem__"]);
}

#[test]
fn getitem_nongeneric_concrete_correct_accepted() {
    let src = "struct SBox:\n\
        \x20   dict[str, int64] m\n\
        \x20   def __getitem__(self, str key) -> int64:\n\
        \x20       return self.m.get(key, 0)\n\n\
         def main():\n\
        \x20   dict[str, int64] d = {}\n\
        \x20   SBox b = SBox(m=d)\n\
        \x20   print(b[\"a\"])\n";
    assert_accepted(src);
}

// ---------------------------------------------------------------------------
// __setitem__ : both the index AND the value argument are checked.
// ---------------------------------------------------------------------------

/// A generic `Box[K]` supporting `b[k]` and `b[k] = v` over a `dict[K, int64]`.
/// Both dunders are defined (sema requires `__getitem__` to index-assign, and the
/// augmented form `b[k] += v` reads then writes), so the only thing under test is
/// the argument check, not dunder presence.
fn setitem_prog(inst: &str, assign: &str) -> String {
    format!(
        "struct Box[K]:\n\
        \x20   dict[K, int64] m\n\
        \x20   def __getitem__(self, K key) -> int64:\n\
        \x20       return self.m.get(key, 0)\n\
        \x20   def __setitem__(self, K key, int64 val):\n\
        \x20       self.m[key] = val\n\n\
         def main():\n\
        \x20   dict[{inst}, int64] d = {{}}\n\
        \x20   Box[{inst}] b = Box(m=d)\n\
        \x20   {assign}\n"
    )
}

#[test]
fn setitem_wrong_index_rejected() {
    // `b[5] = 0` on a `Box[str]`: the key is a scalar where a pointer is wanted.
    assert_rejected(
        &setitem_prog("str", "b[5] = 0"),
        "ZAP4021",
        &["__setitem__"],
    );
}

#[test]
fn setitem_wrong_value_rejected() {
    // `b["a"] = "x"` on a `Box[str]`: the value is a pointer where the `int64`
    // value parameter is wanted — the value argument is checked, not just the
    // index.
    assert_rejected(
        &setitem_prog("str", "b[\"a\"] = \"x\""),
        "ZAP4021",
        &["__setitem__"],
    );
}

#[test]
fn setitem_correct_accepted() {
    assert_accepted(&setitem_prog("str", "b[\"a\"] = 1"));
}

#[test]
fn setitem_wrong_index_typed_binding_rejected() {
    // The typed-binding form of a `__setitem__` index mismatch is caught too.
    assert_rejected(
        &setitem_prog("str", "int64 k = 1\n    b[k] = 0"),
        "ZAP4021",
        &["__setitem__"],
    );
}
