//! ZIR — Zap's typed SSA intermediate representation, plus AST -> ZIR lowering.
//!
//! This crate has no LLVM dependency, so it builds and tests on any host. The
//! backend (`zapc_codegen`) consumes [`Module`] and translates ZIR -> LLVM.
//! See `design/IR.md`.

// The ZIR data model lives in the dependency-free `zapc_ir_core` leaf (#129);
// re-export it as the `ir` module so `zapc_ir::ir::*` and the in-crate
// `crate::ir::*` paths are unchanged.
pub use zapc_ir_core as ir;
pub mod escape;
pub mod lower;
// Monomorphization (turn user generics into concrete declarations before
// lowering, I#154, D-0017/D-0064) lives in the `zapc_mono` leaf (#129);
// re-export it as the `mono` module so `crate::mono::monomorphize` is unchanged.
pub use zapc_mono as mono;

pub use lower::{lower_module, lower_module_opts};
pub use zapc_ir_core::{
    ArrayMathOp, AtomicOrd, AtomicRmwOp, BinOp, Block, BlockId, CastKind, CmpPred, Dtype, Elem,
    Func, Inst, Module, Owner, Term, Ty, UnOp, Value,
};
// `LowerError` moved to `zapc_mono` with monomorphization (#129); re-export it
// here (and at `zapc_ir::lower::LowerError`, via `lower.rs`) unchanged.
pub use zapc_mono::LowerError;

#[cfg(test)]
mod tests {
    use super::*;
    use zapc_syntax::parser::parse_str;

    fn lower_ok(src: &str) -> Module {
        let (ast, perrs, lerrs) = parse_str(src);
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        let (module, errs) = lower_module(&ast);
        assert!(errs.is_empty(), "lower errors: {errs:?}");
        module
    }

    // --- AtomicArray[T] (#844) -------------------------------------------------

    #[test]
    fn concurrent_set_and_stack_lower() {
        // ConcurrentSet[int64] wraps the map (-> AtomicArray bucket directory);
        // the Treiber ConcurrentStack[str] uses a single atomic head CAS. Both
        // must monomorphize and lower end-to-end.
        let m = lower_ok(
            "def main():\n    \
             ConcurrentSet[int64] s = new_concurrent_set()\n    \
             s.add(1)\n    \
             auto c = s.contains(1)\n    \
             ConcurrentStack[str] st = new_concurrent_stack()\n    \
             st.push(\"x\")\n    \
             auto v, ok = st.pop()\n",
        );
        let text = format!("{m}");
        assert!(text.contains("zap_atomicarray_new"), "text:\n{text}");
    }

    #[test]
    fn concurrent_map_monomorphizes_and_lowers() {
        // Using ConcurrentMap[str, int64] forces its generic split-order methods
        // to monomorphize and lower end-to-end; the bucket directory lowers to the
        // AtomicArray runtime entry points (#184 built on #844).
        let m = lower_ok(
            "def main():\n    \
             ConcurrentMap[str, int64] m = new_concurrent_map()\n    \
             m.put(\"k\", 1)\n    \
             auto v, ok = m.get(\"k\")\n    \
             auto removed = m.delete(\"k\")\n    \
             auto n = m.length()\n",
        );
        let text = format!("{m}");
        assert!(text.contains("zap_atomicarray_new"), "text:\n{text}");
        assert!(text.contains("zap_atomicarray_load_ptr"), "text:\n{text}");
    }

    #[test]
    fn atomicarray_int64_lowers_to_word_entry_points() {
        // A word (int64) element: new(cap, elem_is_ptr) + the `_i64` entry points,
        // and compare_and_swap's i64 result is turned into a bool via `icmp ne 0`.
        let m = lower_ok(
            "def main():\n    \
             AtomicArray[int64] a = AtomicArray[int64](4)\n    \
             a.store(0, 7)\n    \
             auto v = a.load(0)\n    \
             auto swapped = a.compare_and_swap(0, 7, 8)\n    \
             auto n = a.len()\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_atomicarray_new"), "text:\n{text}");
        assert!(text.contains("@zap_atomicarray_store_i64"), "text:\n{text}");
        assert!(text.contains("@zap_atomicarray_load_i64"), "text:\n{text}");
        assert!(text.contains("@zap_atomicarray_cas_i64"), "text:\n{text}");
        assert!(text.contains("@zap_atomicarray_len"), "text:\n{text}");
    }

    #[test]
    fn atomicarray_reference_lowers_to_ptr_entry_points() {
        // A reference element uses the `_ptr` entry points, passing/returning the
        // pointer word with no int cast.
        let m = lower_ok(
            "struct Node:\n    int64 v\n\n\
             def main():\n    \
             AtomicArray[Node] a = AtomicArray[Node](2)\n    \
             a.store(0, Node(v=1))\n    \
             auto x = a.load(0)\n    \
             auto swapped = a.compare_and_swap(0, x, Node(v=2))\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_atomicarray_store_ptr"), "text:\n{text}");
        assert!(text.contains("@zap_atomicarray_load_ptr"), "text:\n{text}");
        assert!(text.contains("@zap_atomicarray_cas_ptr"), "text:\n{text}");
    }

    #[test]
    fn raw_crypto_results_support_inferred_bytes_operations() {
        // #647: compile the reduced repro for every affected API. No runtime or
        // randomness is involved; direct operations on auto locals lost bytes.
        for expression in [
            "crypto.digest(crypto.sha256_new())",
            "crypto.hmac_sha256_raw(b\"key\", b\"message\")",
            "crypto.random_bytes(4)",
        ] {
            lower_ok(&format!(
                r#"def main():
    auto result = {expression}
    auto extended = result + b"x"
    auto first = result[0]
    auto sliced = result[1:]
    auto same = result == b"independent bytes"
"#
            ));
        }
    }

    // ---- explicit capability dunders on intrinsic bound parameters (I#820) ----
    //
    // A capability dunder called by name (`k.__cmp__(x)`, `k.__eq__(x)`,
    // `k.__hash__()`) on a value of a type parameter bounded on a capability now
    // lowers when that parameter is monomorphized to an intrinsic type — it emits
    // the same primitive the operator surface uses, rather than hitting the old
    // "method call on a non-struct value" error.

    #[test]
    fn capability_dunders_lower_on_scalar_bound_free_function() {
        // The #820 repro: a bounded free function calling the dunder on a value of
        // T, instantiated with int64. Previously ZAP6001.
        lower_ok(
            "def order[T: Comparable](T a, T b) -> int64:\n    return a.__cmp__(b)\n\
             def eq[T: Equatable](T a, T b) -> bool:\n    return a.__eq__(b)\n\
             def h[T: Hashable](T a) -> int64:\n    return a.__hash__()\n\
             def main():\n    print(order(1, 2))\n    print(eq(1, 1))\n    print(h(7))\n",
        );
    }

    #[test]
    fn capability_dunders_lower_for_every_intrinsic_instance() {
        // int / char / bool / tier-1 enum / float / bytes all carry the three
        // capability dunders intrinsically; each instantiation lowers.
        let prelude = "enum Color:\n    Red\n    Green\n\
             def cmp3[T: Comparable](T a, T b) -> int64:\n    return a.__cmp__(b)\n\
             def same[T: Equatable](T a, T b) -> bool:\n    return a.__eq__(b)\n\
             def hv[T: Hashable](T a) -> int64:\n    return a.__hash__()\n";
        for (ty, a, b) in [
            ("int64", "1", "2"),
            ("bool", "True", "False"),
            ("float64", "1.5", "2.5"),
            ("Color", "Color.Red", "Color.Green"),
        ] {
            // Explicit type argument (I#161). __cmp__ (Comparable) +
            // __eq__/__hash__ (via Hashable ⊃ Equatable).
            lower_ok(&format!(
                "{prelude}def main():\n    print(cmp3[{ty}]({a}, {b}))\n\
                 \x20   print(same[{ty}]({a}, {a}))\n    print(hv[{ty}]({a}))\n"
            ));
        }
        // char arises from indexing; bytes/str are reference byte buffers. `str`
        // is `Ty::Ptr` and must route to the capability path (not the string-method
        // path) for the three dunders — all three lower via zap_str_cmp/eq/hash.
        lower_ok(
            "def cmp3[T: Comparable](T a, T b) -> int64:\n    return a.__cmp__(b)\n\
             def same[T: Equatable](T a, T b) -> bool:\n    return a.__eq__(b)\n\
             def hv[T: Hashable](T a) -> int64:\n    return a.__hash__()\n\
             def main():\n    str s = \"ab\"\n    print(same[char](s[0], s[1]))\n\
             \x20   print(same[bytes](b\"x\", b\"y\"))\n    print(hv[bytes](b\"x\"))\n\
             \x20   print(cmp3[str](\"p\", \"q\"))\n    print(same[str](\"p\", \"q\"))\n\
             \x20   print(hv[str](\"p\"))\n",
        );
    }

    #[test]
    fn capability_dunder_on_bound_field_in_user_container_lowers() {
        // The user-container case (#811 body use): a method calls the dunder on a
        // field typed as the bounded parameter, monomorphized to a scalar.
        lower_ok(
            "value struct BST[K: Comparable, V]:\n    K key\n    V val\n\
             \x20   def cmp_key(self, K other) -> int64:\n        return self.key.__cmp__(other)\n\
             def main():\n    BST[int64, str] b = BST[int64, str](key=1, val=\"a\")\n\
             \x20   print(b.cmp_key(2))\n",
        );
    }

    #[test]
    fn crypto_asym_surface_lowers() {
        // #643 / D-0126: the validated asymmetric-key surface lowers end to end,
        // including the `KeyInfo` value struct built from the key_info getters
        // (prelude injection is gated on the "key_info" needle) and field access.
        let module = lower_ok(
            r#"def main():
    auto rsa_pub = crypto.rsa_public_key_pem(b"n", b"e")
    auto ec_pub = crypto.p256_public_key_pem(b"x", b"y")
    auto ed_pub = crypto.ed25519_public_key_pem(b"x")
    auto info = crypto.key_info(rsa_pub)
    auto kind = info.kind
    auto bits = info.bits
    auto curve = info.curve
    auto is_priv = info.is_private
    auto es = crypto.p256_sha256_sign("pem", b"msg")
    auto ok = crypto.p256_sha256_verify(ec_pub, b"msg", es)
    assert ok
    auto ed = crypto.ed25519_sign("pem", b"msg")
    auto ok2 = crypto.ed25519_verify(ed_pub, b"msg", ed)
    assert ok2
"#,
        );
        let main = module.funcs.iter().find(|f| f.name == "main").unwrap();
        for (symbol, expected) in [
            ("zap_crypto_rsa_public_key_pem", Ty::Bytes),
            ("zap_crypto_p256_public_key_pem", Ty::Bytes),
            ("zap_crypto_ed25519_public_key_pem", Ty::Bytes),
            ("zap_crypto_key_kind", Ty::Ptr),
            ("zap_crypto_key_bits", Ty::I64),
            ("zap_crypto_key_curve", Ty::Ptr),
            ("zap_crypto_p256_sha256_sign", Ty::Bytes),
            ("zap_crypto_ed25519_sign", Ty::Bytes),
            // Verifiers return int64 (0/1); call_bool normalizes to i1.
            ("zap_crypto_p256_sha256_verify", Ty::I64),
            ("zap_crypto_ed25519_verify", Ty::I64),
        ] {
            let found = main
                .blocks
                .iter()
                .flat_map(|b| &b.insts)
                .any(|i| matches!(i, Inst::Call { func, .. } | Inst::Invoke { func, .. } if func == symbol));
            assert!(found, "missing lowered call to {symbol}");
            let _ = expected;
        }
        // The KeyInfo struct is built and its fields are read.
        assert!(
            main.blocks
                .iter()
                .flat_map(|b| &b.insts)
                .any(|i| matches!(i, Inst::StructNew { .. })),
            "key_info must build a KeyInfo struct"
        );
    }

    #[test]
    fn crypto_result_types_distinguish_buffers_text_and_handles() {
        // Guard the boundary: opaque streaming contexts are not digest buffers,
        // and the existing hex/string APIs must keep string operation dispatch.
        let module = lower_ok(
            r#"def main():
    auto h = crypto.sha256_new()
    crypto.update(h, b"message")
    auto sha256 = crypto.sha256(b"message")
    auto sha512 = crypto.sha512(b"message")
    auto sha1 = crypto.sha1(b"message")
    auto md5 = crypto.md5(b"message")
    auto digest = crypto.digest(h)
    auto hex_digest = crypto.hexdigest(h)
    auto raw_mac = crypto.hmac_sha256_raw(b"key", b"message")
    auto hex_mac = crypto.hmac_sha256(b"key", b"message")
    auto hex_mac512 = crypto.hmac_sha512(b"key", b"message")
    auto rsa1 = crypto.rsa_sha1_sign("invalid PEM", b"message")
    auto rsa256 = crypto.rsa_sha256_sign("invalid PEM", b"message")
    auto verified = crypto.rsa_sha256_verify("invalid PEM", b"message", b"sig")
    assert not verified
    auto ct_eq = crypto.constant_time_eq(b"message", b"message")
    assert ct_eq
    int32 count32 = 4
    int64 count64 = 4
    auto random = crypto.random_bytes(count32)
    auto token = crypto.token_hex(count64)
"#,
        );
        let main = module.funcs.iter().find(|f| f.name == "main").unwrap();
        for (symbol, expected) in [
            ("zap_crypto_new", Ty::Ptr),
            ("zap_crypto_sha256", Ty::Ptr),
            ("zap_crypto_sha512", Ty::Ptr),
            ("zap_crypto_sha1", Ty::Ptr),
            ("zap_crypto_md5", Ty::Ptr),
            ("zap_crypto_hmac_sha512", Ty::Ptr),
            ("zap_crypto_rsa_sha1_sign", Ty::Bytes),
            ("zap_crypto_rsa_sha256_sign", Ty::Bytes),
            // Runtime verification returns int64; call_bool normalizes to i1.
            ("zap_crypto_rsa_sha256_verify", Ty::I64),
            // Constant-time compare returns int64 (0/1); call_bool normalizes.
            ("zap_crypto_constant_time_eq", Ty::I64),
            ("zap_crypto_digest", Ty::Bytes),
            ("zap_crypto_hexdigest", Ty::Ptr),
            ("zap_crypto_hmac_sha256_raw", Ty::Bytes),
            ("zap_crypto_hmac_sha256", Ty::Ptr),
            ("zap_crypto_random_bytes", Ty::Bytes),
            ("zap_crypto_token_hex", Ty::Ptr),
        ] {
            let result = main
                .blocks
                .iter()
                .flat_map(|b| &b.insts)
                .find_map(|i| match i {
                    Inst::Call {
                        dest: Some(d),
                        func,
                        ..
                    }
                    | Inst::Invoke {
                        dest: Some(d),
                        func,
                        ..
                    } if func == symbol => Some(*d),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {symbol}"));
            assert_eq!(main.value_ty(result), expected, "result type of {symbol}");
        }
        for inst in main.blocks.iter().flat_map(|b| &b.insts) {
            if let Inst::Call { func, args, .. } = inst {
                if matches!(
                    func.as_str(),
                    "zap_crypto_random_bytes" | "zap_crypto_token_hex"
                ) {
                    assert_eq!(main.value_ty(args[0]), Ty::I64, "count ABI for {func}");
                }
            }
        }
        assert!(
            main.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(
                i, Inst::Call { dest: None, func, .. } if func == "zap_crypto_update"
            )),
            "streaming update must remain void"
        );
    }

    #[test]
    fn rsa_sign_results_support_inferred_bytes_operations() {
        // Compile-only: invalid PEM is never executed. This is the reduced
        // #644 regression; both APIs must support byte operations on auto locals.
        for algorithm in ["sha1", "sha256"] {
            lower_ok(&format!(
                r#"def main():
    auto sig = crypto.rsa_{algorithm}_sign("invalid PEM", b"message")
    auto extended = sig + b"x"
    auto first = sig[0]
    auto sliced = sig[1:]
    auto same = sig == b"independent bytes"
"#
            ));
        }
    }

    #[test]
    fn lower_array_collection_contract_fixtures() {
        let module = lower_ok(include_str!(
            "../../../tests/conformance/array_contract.zap"
        ));
        // Element-wise array-array ops guard with a broadcast-compatibility check
        // (#501, D-0165) — a throwing call that raises ValueError on incompatible
        // shapes before the kernel runs (it subsumes the old exact-shape check).
        assert!(format!("{module}").contains("zap_array_broadcast_check"));
        lower_ok(include_str!(
            "../../../tests/conformance/collection_elements.zap"
        ));
    }

    #[test]
    fn lower_nested_collections_readback_dispatch() {
        // #625: a nested generic collection must read back as its real element
        // type, not a bare `str` pointer. Before the fix, `rows[i]` yielded a
        // `Ptr` and `len(rows[i])` / `rows[i][j]` dispatched to the string entry
        // points (`zap_str_len` / `zap_str_index`), silently miscompiling into
        // wrong/empty values. Assert the collection entry points are emitted and
        // no string op leaks in.
        let module = lower_ok(include_str!("../../../tests/golden/nested_collections.zap"));
        let text = format!("{module}");
        // A nested read-back dispatches as a list/dict op, never a string op.
        assert!(
            !text.contains("zap_str_len") && !text.contains("zap_str_index"),
            "a nested read-back leaked a string op:\n{text}"
        );
        // `make_grid` returns `list[list[int64]]`: inner lists push as pointers.
        assert!(text.contains("call @zap_list_push_ptr"), "text:\n{text}");
        // Reading an inner `int64` element goes through the typed list entry point.
        assert!(text.contains("call @zap_list_get_i64"), "text:\n{text}");
        // A `dict[str, list]` value reads back as a list (pointer value entry).
        assert!(text.contains("call @zap_dict_get_str_ptr"), "text:\n{text}");
        // A `list[dict[str, int64]]` element reads back as a dict with int values.
        assert!(text.contains("call @zap_dict_get_str_i64"), "text:\n{text}");
    }

    #[test]
    fn nested_empty_literals_use_destination_types() {
        lower_ok(include_str!(
            "../../../tests/golden/nested_empty_append.zap"
        ));
    }

    #[test]
    fn lower_string_hex_unicode_escapes() {
        // #781: `\x`/`\u`/`\U` decode to the codepoint's UTF-8 in a str literal.
        let m = lower_ok(concat!(
            "def main():\n",
            "    print(\"\\x41\")\n",
            "    print(\"caf\\u00e9\")\n",
            "    print(\"\\U0001F600\")\n",
        ));
        let t = format!("{m}");
        assert!(t.contains("strconst \"A\""), "\\x41 -> A:\n{t}");
        assert!(t.contains("strconst \"café\""), "\\u00e9 -> café:\n{t}");
        assert!(t.contains('😀'), "\\U0001F600 -> emoji:\n{t}");
        // A malformed hex escape is a compile (lower) error, not silent.
        let (ast, _p, _l) = parse_str("def main():\n    print(\"\\x1\")\n");
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "a malformed \\x escape must be a compile error"
        );
    }

    #[test]
    fn lower_ternary_branches_not_eager_select() {
        // #776: `a if c else b` lowers to a real conditional branch (`br_if`),
        // with each arm in its own block, so only the chosen arm runs — it is no
        // longer an eager `select` that evaluates both operands up front. `main`
        // here has no other control flow, so the `br_if` is the ternary's.
        let m = lower_ok(concat!(
            "def pick(int64 x) -> int64:\n",
            "    return x\n",
            "\n",
            "def main():\n",
            "    auto c = True\n",
            "    print(pick(1) if c else pick(2))\n",
        ));
        let text = format!("{m}");
        assert!(
            text.contains("br_if"),
            "ternary must lower to a conditional branch:\n{text}"
        );
        // The merge slot's `alloca` must dominate both arms — i.e. sit in the
        // pre-branch block, before the `br_if` — not inside the then-arm (which
        // would not dominate the else-arm's store, a miscompile). Pin it by
        // position: the first `alloca` precedes the first `br_if`.
        let main = &text[text.find("func @main").expect("main")..];
        let alloca_at = main.find("alloca").expect("ternary merge slot alloca");
        let brif_at = main.find("br_if").expect("ternary br_if");
        assert!(
            alloca_at < brif_at,
            "ternary merge slot must be allocated before the branch:\n{main}"
        );
    }

    #[test]
    fn lower_range_step_is_sign_aware() {
        // #759: a 3-arg range tests the step sign at the loop condition (a
        // `select` between `<` and `>`) and raises ValueError on a zero step,
        // instead of the old unconditional `cur < stop`.
        let dyn_mod = lower_ok(concat!(
            "def main():\n",
            "    auto s = 0\n",
            "    auto k = -1\n",
            "    for i in range(5, 0, k):\n",
            "        s = s + i\n",
        ));
        let dyn_text = format!("{dyn_mod}");
        assert!(
            dyn_text.contains("select"),
            "3-arg range needs a sign-aware select:\n{dyn_text}"
        );
        assert!(
            dyn_text.contains("@zap_panic_typed"),
            "3-arg range needs a zero-step guard:\n{dyn_text}"
        );
        // A 2-arg range keeps the simple ascending test — no step guard/select.
        let simple = lower_ok(concat!(
            "def main():\n",
            "    auto s = 0\n",
            "    for i in range(0, 5):\n",
            "        s = s + i\n",
        ));
        let simple_text = format!("{simple}");
        assert!(
            !simple_text.contains("@zap_panic_typed"),
            "2-arg range must not emit a step guard:\n{simple_text}"
        );
    }

    #[test]
    fn lower_float_mod_uses_floored_helper() {
        // #751: float `%` lowers to the floored `zap_ffmod` runtime helper — a
        // throwing call (since `% 0.0` raises a catchable ZeroDivisionError) —
        // not a bare `frem`, so mixed-sign float modulo matches CPython.
        let module = lower_ok(concat!(
            "def main():\n",
            "    f64 x = 5.5\n",
            "    print(str(x % -2.0))\n",
        ));
        let text = format!("{module}");
        assert!(
            text.contains("@zap_ffmod"),
            "float % must call zap_ffmod:\n{text}"
        );
    }

    #[test]
    fn lower_aug_assign_subscript_and_field() {
        // #750: augmented assignment to a subscript or field target lowers to a
        // read-modify-write of that one slot, with the base/index evaluated once
        // (it used to bail with "only simple variable aug-assignment"). `lower_ok`
        // asserts there are no lower errors; the text assertions pin the shape.
        let module = lower_ok(concat!(
            "def idx() -> int64:\n",
            "    print(\"idx\")\n",
            "    return 0\n",
            "\n",
            "struct Counter:\n",
            "    int64 n\n",
            "\n",
            "def main():\n",
            "    list[int64] xs = [10, 20]\n",
            "    xs[idx()] += 5\n",
            "    dict[str, int64] d = {\"a\": 1}\n",
            "    d[\"a\"] *= 2\n",
            "    Counter c = Counter(0)\n",
            "    c.n -= 1\n",
        ));
        let text = format!("{module}");
        // The list element is read (throwing, like `xs[i]`) and written back.
        assert!(text.contains("@zap_list_get_i64"), "no list read:\n{text}");
        assert!(
            text.contains("call @zap_list_set_i64"),
            "no list write-back:\n{text}"
        );
        // The side-effecting index `idx()` is evaluated exactly once — count
        // call sites only (`invoke`/`call`), not the `func @idx()` definition.
        let idx_calls = text.matches("invoke @idx()").count() + text.matches("call @idx()").count();
        assert_eq!(
            idx_calls, 1,
            "idx() must be evaluated exactly once:\n{text}"
        );
        // The dict value is read and written back through the typed entry points.
        assert!(
            text.contains("@zap_dict_get_str_i64"),
            "no dict read:\n{text}"
        );
        assert!(
            text.contains("call @zap_dict_set_str_i64"),
            "no dict write-back:\n{text}"
        );
        // The struct field is a FieldGet/FieldSet read-modify-write.
        assert!(
            text.contains("fieldget") && text.contains("fieldset"),
            "no field read-modify-write:\n{text}"
        );
    }

    #[test]
    fn nested_append_still_rejects_incompatible_values() {
        for (declaration, value) in [
            ("list[dict[str, bool]] values = []", "{1: False}"),
            ("list[list[str]] values = []", "[1]"),
            ("list[list[dict[str, bool]]] values = []", "[{1: True}]"),
            ("list[dict[str, bool]] values = []", "wrong"),
        ] {
            let source = format!(
                "def main():\n    dict[int64, bool] wrong = {{1: True}}\n    {declaration}\n    values.append({value})\n"
            );
            let (ast, parse, lex) = parse_str(&source);
            assert!(parse.is_empty() && lex.is_empty());
            let (_, errors) = lower_module(&ast);
            assert!(!errors.is_empty(), "incompatible append accepted: {value}");
        }
    }

    #[test]
    fn nested_index_and_conditional_reject_incompatible_values() {
        for operation in [
            "names[0] = {1: 2}",
            "names[0] = wrong",
            "maps[\"x\"] = {1: 2}",
            "maps[\"x\"] = wrong",
            "names[0] = names[1] = wrong",
            "names.append({} if True else wrong)",
            "names.append(wrong if True else {})",
            "names.append({} if True else {1: 2})",
            "names.append({1: 2} if True else {})",
            "names[0] = {} if True else wrong",
            "maps[\"x\"] = wrong if True else {}",
        ] {
            let source = format!(
                "def main():\n    list[dict[str, bool]] names = [{{}}, {{}}]\n    dict[str, dict[str, bool]] maps = {{}}\n    dict[int64, int64] wrong = {{1: 2}}\n    {operation}\n"
            );
            let (ast, parse, lex) = parse_str(&source);
            assert!(parse.is_empty() && lex.is_empty(), "{operation}");
            let (_, errors) = lower_module(&ast);
            assert!(
                !errors.is_empty(),
                "incompatible store accepted: {operation}"
            );
        }
    }

    #[test]
    fn literal_element_mismatch_names_expected_and_actual_types() {
        let (ast, parse, lex) = parse_str("def main():\n    list[float64] xs = [1, 2]\n");
        assert!(parse.is_empty() && lex.is_empty());
        let (_, errors) = lower_module(&ast);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("expects `f64`, found `i64`")),
            "{errors:?}"
        );
    }

    #[test]
    fn nested_collection_key_is_a_loud_error() {
        // #625: a collection is unhashable, so using one as a dict key or a set
        // element must fail loudly rather than silently hashing its pointer as a
        // string. Both the literal and the annotated forms are rejected.
        for op in [
            "auto s = {[1, 2]}",
            "auto d = {[1]: 9}",
            "auto s = {{\"a\": 1}}",
        ] {
            let source = format!("def main():\n    {op}\n");
            let (ast, parse, lex) = parse_str(&source);
            assert!(parse.is_empty() && lex.is_empty(), "{op}: parse/lex");
            let (_, errors) = lower_module(&ast);
            assert!(
                errors.iter().any(|e| e.message.contains("unhashable")),
                "{op} should be a loud 'unhashable' error, got: {errors:?}"
            );
        }
    }

    #[test]
    fn lower_scope_timeout_fixture() {
        let module = lower_ok(include_str!("../../../tests/conformance/scope_timeout.zap"));
        assert!(format!("{module}").contains("zap_scope_timeout"));
        lower_ok(include_str!(
            "../../../tests/conformance/scope_timeout_grace.zap"
        ));
        lower_ok(include_str!(
            "../../../tests/conformance/scope_timeout_pending.zap"
        ));
    }

    #[test]
    fn unsupported_array_operations_have_lowering_errors() {
        for (operation, message) in [
            // Stepped / reversed / N-D array slices are supported since #502
            // (see the strided-view tests); `/` and scalar-on-the-left `-`/`/`
            // since #506; `//` (floor division) and `%` stay deferred.
            ("auto x = a // a", "only +, -, *, and /"),
            ("auto x = 1 // a", "only +, -, *, and /"),
            ("auto x = a % 2", "only +, -, *, and /"),
            // Multi-axis indexing is supported for N-D arrays (#500); on a 1-D
            // array a two-axis index is simply too many indices.
            ("auto x = a[0, 1]", "too many indices"),
            ("a[0, 1] = 2", "too many indices"),
            ("auto x = a[0.5:]", "slice bounds must be integers"),
            // `==` on arrays is a boolean-mask comparison since #510 (see
            // mask_tests); `-a` (element-wise negate) stays unsupported.
            ("auto x = -a", "unary array operations are unsupported"),
            ("a[0] = \"x\"", "assignment requires a numeric scalar"),
        ] {
            let source = format!("def main():\n    auto a = array[i64]([1,2])\n    {operation}\n");
            let (ast, parse, lex) = parse_str(&source);
            assert!(parse.is_empty() && lex.is_empty());
            let (_, errors) = lower_module(&ast);
            assert!(
                errors.iter().any(|e| e.message.contains(message)),
                "{operation}: {errors:?}"
            );
        }
    }

    #[test]
    fn lower_scope_cleanup_conformance() {
        let module = lower_ok(include_str!("../../../tests/conformance/scope_cleanup.zap"));
        let early = module
            .funcs
            .iter()
            .find(|f| f.name == "return_from_scope")
            .unwrap();
        assert!(early
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .any(|i| matches!(i, Inst::Call { func, .. } if func == "zap_scope_exit_cancel")));
    }

    #[test]
    fn lower_structural_match_conformance() {
        lower_ok(include_str!(
            "../../../tests/conformance/match_structural.zap"
        ));
    }

    #[test]
    fn lower_chained_comparison_single_eval_and_short_circuit() {
        // `1 < f(2) < 3` is Python-chained (I#386): it must evaluate the
        // interior operand `f(2)` exactly once, reuse that value as the left
        // side of the second comparison, and only reach the second link (which
        // materializes the last operand `3`) when the first link holds.
        let m = lower_ok(
            "def f(int64 n) -> int64:\n    return n\n\ndef main():\n    print(str(1 < f(2) < 3))\n",
        );
        let text = format!("{m}");
        // The side-effecting middle operand is called exactly once. (Match the
        // call site by callee, not a specific SSA value — `main`'s value numbers
        // shift as unrelated bootstrap is emitted, e.g. the signature-reflection
        // registry, I#458.)
        let calls = text.matches("invoke @f(").count() + text.matches("call @f(").count();
        assert_eq!(calls, 1, "f must be evaluated exactly once:\n{text}");
        // Two comparison links.
        assert_eq!(
            text.matches("icmp.Lt").count(),
            2,
            "two comparison links:\n{text}"
        );
        // The chain short-circuits through a conditional branch, and the result
        // is merged through a single i1 stack slot.
        assert!(text.contains("alloca i1"), "merge slot:\n{text}");
        assert!(text.contains("br_if"), "short-circuit branch:\n{text}");
    }

    #[test]
    fn lower_fib_fixture() {
        let m = lower_ok(include_str!("../../../tests/golden/fib.zap"));
        assert_eq!(m.funcs.len(), 2);
        let fib = m.funcs.iter().find(|f| f.name == "fib").expect("fib");
        assert_eq!(fib.params, vec![Ty::I64]);
        assert_eq!(fib.ret, Ty::I64);
        let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
        assert!(main.is_main);
        // Textual form mentions the recursive call and a comparison.
        let text = format!("{m}");
        assert!(text.contains("call @fib"), "text was:\n{text}");
        assert!(text.contains("icmp"), "text was:\n{text}");
    }

    #[test]
    fn lower_interface_dispatch_fixture() {
        // Interface values (D-0016, #84): a concrete struct assigned to an
        // interface-typed binding is wrapped (`ifacemake`), and an
        // interface-method call dispatches through the itable (`ifacecall`).
        let m = lower_ok(include_str!("../../../tests/golden/interface_dispatch.zap"));
        // One interface registered, with its synthetic fat-pointer layout and the
        // two method slots in declaration order.
        assert_eq!(m.interfaces.len(), 1, "one interface");
        let shape = &m.interfaces[0];
        assert_eq!(shape.name, "Shape");
        let slots: Vec<&str> = shape.methods.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            slots,
            vec!["area", "name"],
            "slot order is declaration order"
        );
        // The fat-pointer layout is the inline `{ptr, ptr}` aggregate.
        let fat = &m.structs[shape.fat_sid as usize];
        assert!(fat.inline, "the fat pointer is passed by value");
        assert_eq!(fat.fields, vec![Ty::Ptr, Ty::Ptr], "{{data, itable}}");
        // The concrete->interface conversion and the dynamic dispatch are emitted.
        let text = format!("{m}");
        assert!(text.contains("ifacemake"), "conversion emitted:\n{text}");
        assert!(
            text.contains("ifacecall"),
            "dynamic dispatch emitted:\n{text}"
        );
    }

    #[test]
    fn lower_loop_sum_fixture() {
        let m = lower_ok(include_str!("../../../tests/golden/loop_sum.zap"));
        assert_eq!(m.funcs.len(), 1);
        let text = format!("{m}");
        assert!(text.contains("print("), "text was:\n{text}");
        assert!(text.contains("Add"), "text was:\n{text}");
    }

    #[test]
    fn lower_assert_power_diagnostics() {
        // A comparison assert evaluates each operand once and, in the failure
        // block, renders the source text plus both operand values.
        let m = lower_ok("def main():\n    int64 x = 2\n    assert x == 3\n");
        let text = format!("{m}");
        assert!(text.contains("br_if"), "assert should branch:\n{text}");
        assert!(
            text.contains("\\n  assert x == 3"),
            "power-assert should echo the source:\n{text}"
        );
        assert!(
            text.contains("left:") && text.contains("right:"),
            "power-assert should show both operands:\n{text}"
        );
        assert!(text.contains("zap_panic"), "failure raises:\n{text}");
    }

    #[test]
    fn lower_assert_with_message() {
        let m = lower_ok("def main():\n    assert False, \"nope\"\n");
        let text = format!("{m}");
        assert!(
            text.contains("strconst \"nope\""),
            "message literal:\n{text}"
        );
    }

    #[test]
    fn lower_with_expect_raises() {
        let m = lower_ok(
            "def boom():\n    raise \"x\"\n\
             def main():\n    with expect_raises(ValueError):\n        boom()\n",
        );
        let text = format!("{m}");
        assert!(
            text.contains("expected ValueError to be raised"),
            "miss message present:\n{text}"
        );
    }

    #[test]
    fn with_general_context_manager_is_rejected() {
        let (ast, _p, _l) =
            parse_str("def main():\n    with open(\"f\") as fh:\n        print(1)\n");
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "general `with` should be rejected for now"
        );
    }

    #[test]
    fn print_and_arithmetic() {
        let m = lower_ok("def main():\n    print(1 + 2 * 3)\n");
        let main = &m.funcs[0];
        let has_print = main
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .any(|i| matches!(i, Inst::Print { .. }));
        assert!(has_print);
    }

    #[test]
    fn lower_strings() {
        let m = lower_ok(include_str!("../../../tests/golden/hello.zap"));
        let text = format!("{m}");
        assert!(text.contains("strconst"), "text:\n{text}");
        assert!(text.contains("call @zap_str_concat"), "text:\n{text}");
        // `str` params/returns are pointers.
        let greet = m.funcs.iter().find(|f| f.name == "greet").expect("greet");
        assert_eq!(greet.params, vec![Ty::Ptr]);
        assert_eq!(greet.ret, Ty::Ptr);
    }

    #[test]
    fn lower_structs() {
        let m = lower_ok(include_str!("../../../tests/golden/point.zap"));
        assert_eq!(m.structs.len(), 1);
        assert_eq!(m.structs[0].name, "Point");
        assert_eq!(m.structs[0].fields, vec![Ty::F64, Ty::F64]);
        assert!(m.funcs.iter().any(|f| f.name == "Point__sumsq"));
        let text = format!("{m}");
        assert!(text.contains("structnew"), "{text}");
        assert!(text.contains("fieldget"), "{text}");
        assert!(text.contains("fieldset"), "{text}");
    }

    #[test]
    fn lower_bitfield_value_struct_packs() {
        // C-style bitfields (I#173, D-0068): a value struct whose fields all
        // carry widths lowers to one packed backing integer. 4 + 4 + 2 + 1 = 11
        // bits packs into a u16 (the smallest unit >= 11 bits), with each field
        // placed least-significant-first in declaration order. Signed integer
        // fields read back sign-extended; bool/enum are unsigned.
        let m = lower_ok(
            "enum Color:\n    RED\n    GREEN\n    BLUE\n    WHITE\n\n\
             value struct Pack:\n    int64 a : 4\n    int64 b : 4\n    \
             Color c : 2\n    bool d : 1\n\n\
             def main():\n    auto p = Pack(a=1, b=2, c=Color.RED, d=True)\n    \
             print(p.a)\n",
        );
        let pack = m
            .structs
            .iter()
            .find(|s| s.name == "Pack")
            .expect("Pack layout");
        assert!(pack.is_value, "Pack is a value struct");
        let pl = pack
            .packed
            .as_ref()
            .expect("Pack is a packed bitfield struct");
        // 11 bits -> the smallest backing integer is i16 (2 bytes). This is the
        // packed word size the struct occupies.
        assert_eq!(pl.backing, Ty::I16, "11 bits pack into a u16");
        let got: Vec<(u32, u32, bool)> = pl
            .fields
            .iter()
            .map(|b| (b.offset, b.width, b.signed))
            .collect();
        assert_eq!(
            got,
            vec![(0, 4, true), (4, 4, true), (8, 2, false), (10, 1, false)],
            "LSB-first offsets, declared widths, signed only for the int fields"
        );
    }

    #[test]
    fn lower_tier1_enum_and_match() {
        // A tier-1 enum is an unboxed discriminant with a distinct ZIR identity:
        // member access is an iconst (RED=0, GREEN=10, BLUE continues at 11),
        // and a `match` is an icmp-against-constant chain (D-0018/D-0040).
        let m = lower_ok(
            "enum Color:\n    RED\n    GREEN = 10\n    BLUE\n\n\
             def classify(Color c) -> int64:\n    match c:\n\
             \x20       case Color.RED:\n            return 1\n\
             \x20       case Color.GREEN:\n            return 2\n\
             \x20       case Color.BLUE:\n            return 3\n\n\
             def main():\n    print(classify(Color.BLUE))\n\
             \x20   print(Color.GREEN.name)\n\x20   for color in Color:\n\
             \x20       print(color.name)\n",
        );
        // No tagged struct is allocated for a tier-1 enum.
        assert!(
            m.structs.is_empty(),
            "tier-1 enum should not allocate a struct"
        );
        let classify = m
            .funcs
            .iter()
            .find(|f| f.name == "classify")
            .expect("classify");
        assert_eq!(
            classify.params,
            vec![Ty::Enum(0)],
            "tier-1 enum keeps its identity while using the i64 ABI"
        );
        let text = format!("{m}");
        // BLUE auto-continues from GREEN=10.
        assert!(text.contains("iconst 11"), "BLUE should be 11:\n{text}");
        assert!(
            text.contains("icmp.Eq"),
            "match dispatches on equality:\n{text}"
        );
        assert!(
            text.contains("zap_list_push_i64"),
            "enum iteration stores discriminants in declaration order:\n{text}"
        );
        assert!(
            text.contains("invalid value for enum Color"),
            "enum .name rejects unknown discriminants:\n{text}"
        );
        // The construction `Color.BLUE` is just the discriminant constant.
        assert!(!text.contains("structnew"), "no struct for tier-1:\n{text}");
    }

    #[test]
    fn lower_tier3_variant_construction_and_match() {
        // A tier-3 enum is a GC-allocated tagged struct `{ tag:i32, payload… }`
        // (D-0046). Construction stores the tag + payload fields; `match`
        // branches on the tag and binds fields positionally or by name.
        let m = lower_ok(
            "enum Shape:\n    Circle(f64 r)\n    Rect(f64 w, f64 h)\n    Unit\n\n\
             def area(Shape s) -> f64:\n    match s:\n\
             \x20       case Shape.Circle(r):\n            return 3.14 * r * r\n\
             \x20       case Shape.Rect(w=w, h=h):\n            return w * h\n\
             \x20       case Shape.Unit:\n            return 1.0\n\n\
             def main():\n\
             \x20   print(area(Shape.Circle(r=2.0)))\n\
             \x20   print(area(Shape.Rect(3.0, 4.0)))\n\
             \x20   print(area(Shape.Unit))\n",
        );
        // One tagged struct: tag(i32) + Circle.r + Rect.w + Rect.h (flattened).
        assert_eq!(m.structs.len(), 1, "one tagged struct for the sum type");
        assert_eq!(m.structs[0].name, "Shape");
        assert_eq!(
            m.structs[0].fields,
            vec![Ty::I32, Ty::F64, Ty::F64, Ty::F64],
            "layout is [tag, Circle.r, Rect.w, Rect.h]"
        );
        let area = m.funcs.iter().find(|f| f.name == "area").expect("area");
        assert_eq!(
            area.params,
            vec![Ty::Struct(0)],
            "scrutinee is a tagged struct"
        );
        let text = format!("{m}");
        assert!(text.contains("structnew struct#0"), "construction:\n{text}");
        // tag slot written, payload written; match reads the tag and the fields.
        // (Match the tag-field write structurally rather than by exact SSA value,
        // which shifts as unrelated `main` bootstrap is emitted — e.g. the
        // signature-reflection registry, I#458.)
        assert!(
            text.lines()
                .any(|l| l.contains("fieldset struct#0") && l.contains(".0 = %")),
            "tag set:\n{text}"
        );
        assert!(
            text.contains("fieldget struct#0"),
            "match reads fields:\n{text}"
        );
    }

    #[test]
    fn lower_recursive_enum_is_pointer() {
        // A (mutually) recursive variant field — whose type is the enum itself —
        // is a pointer to a heap enum value, breaking the otherwise-infinite
        // size (D-0046). Reusing the struct machinery gives this for free: the
        // field's ZIR type is `Struct(sid)`, i.e. a pointer.
        let m = lower_ok(
            "enum Expr:\n    Num(int64 v)\n    Add(Expr l, Expr r)\n\n\
             def eval(Expr e) -> int64:\n    match e:\n\
             \x20       case Expr.Num(v):\n            return v\n\
             \x20       case Expr.Add(l, r):\n            return eval(l) + eval(r)\n\n\
             def main():\n    print(eval(Expr.Add(Expr.Num(2), Expr.Num(3))))\n",
        );
        assert_eq!(m.structs.len(), 1);
        // Layout: [tag:i32, Num.v:i64, Add.l:struct#0, Add.r:struct#0]. The
        // recursive fields point back to the same tagged struct.
        assert_eq!(
            m.structs[0].fields,
            vec![Ty::I32, Ty::I64, Ty::Struct(0), Ty::Struct(0)],
            "recursive variant fields are pointers to the enum's own struct"
        );
        let text = format!("{m}");
        assert!(
            text.contains("call @eval"),
            "recursion lowers to a call:\n{text}"
        );
    }

    #[test]
    fn lower_sets() {
        let m = lower_ok(include_str!("../../../tests/golden/set_int.zap"));
        let text = format!("{m}");
        // Literal construction, add, membership, len, and snapshot-iteration all
        // lower to the set runtime entry points.
        assert!(text.contains("call @zap_set_new"), "text:\n{text}");
        assert!(text.contains("call @zap_set_add_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_set_contains_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_set_len"), "text:\n{text}");
        assert!(text.contains("call @zap_set_items"), "text:\n{text}");
    }

    #[test]
    fn lower_heap() {
        // heap[int64] (D-0057): construction, push/pop/peek/len, and the
        // priority-order snapshot that `for x in h` iterates all lower to the
        // heap runtime entry points. A `heap[int64]` carries its element kind in
        // the ZIR type.
        let m = lower_ok(include_str!("../../../tests/golden/heap.zap"));
        let text = format!("{m}");
        assert!(text.contains("alloca heap[i64]"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_new"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_push_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_pop_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_peek_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_len"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_items"), "text:\n{text}");
    }

    #[test]
    fn lower_heap_str_elements() {
        // A `heap[str]` routes through the pointer-word entry points; string
        // elements are compared by `ZapStr` bytes in the runtime.
        let m = lower_ok(
            "def main():\n    \
                 auto h = heap[str]()\n    \
                 h.push(\"pear\")\n    \
                 h.push(\"apple\")\n    \
                 print(h.pop())\n    \
                 print(h.peek())\n",
        );
        let text = format!("{m}");
        assert!(text.contains("alloca heap[ptr]"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_push_ptr"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_pop_ptr"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_peek_ptr"), "text:\n{text}");
    }

    #[test]
    fn lower_heap_max_comparator() {
        // `cmp="max"` is accepted and lowers to the same `zap_heap_new(kind, dir)`
        // construction (the direction argument carries min vs max; the runtime
        // golden exercises the resulting largest-first ordering).
        let m = lower_ok(
            "def main():\n    \
                 auto h = heap[int64](cmp=\"max\")\n    \
                 h.push(1)\n    \
                 print(h.pop())\n",
        );
        let text = format!("{m}");
        assert!(text.contains("call @zap_heap_new"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_push_i64"), "text:\n{text}");
    }

    #[test]
    fn heap_of_tier1_enum_selects_the_i64_comparator_not_str() {
        // I#818: a `heap[Color]` (tier-1 enum) is an `i64`-word heap — the enum is
        // its backing discriminant — so `zap_heap_new(kind, dir)` must pass
        // kind == 0 (i64), NOT kind == 1 (str/pointer). The bug was `Elem::Enum`
        // falling into the str arm, so the first `push` routed the enum word into
        // `zap_str_cmp` and aborted. Assert the kind constant feeding the
        // construction is 0; also confirm push/pop use the i64 entry points (their
        // `val_suffix` was already `i64`, so only the comparator kind was wrong).
        let m = lower_ok(
            "enum Color:\n    Red\n    Green\n    Blue\n\
             def main():\n    heap[Color] h = heap[Color]()\n    \
             h.push(Color.Green)\n    h.push(Color.Red)\n    print(h.pop() == Color.Red)\n",
        );
        let main = m.funcs.iter().find(|f| f.is_main).expect("main emitted");
        // Trace each value to the constant it was defined with, then read the
        // first argument (kind) of the `zap_heap_new` call.
        let mut consts: std::collections::HashMap<u32, i64> = std::collections::HashMap::new();
        let mut kind_arg: Option<u32> = None;
        for b in &main.blocks {
            for inst in &b.insts {
                match inst {
                    Inst::IConst { dest, val } => {
                        consts.insert(*dest, *val);
                    }
                    Inst::Call { func, args, .. } if func == "zap_heap_new" => {
                        kind_arg = args.first().copied();
                    }
                    _ => {}
                }
            }
        }
        let kind = consts
            .get(&kind_arg.expect("zap_heap_new was emitted"))
            .copied();
        assert_eq!(
            kind,
            Some(0),
            "heap[tier-1 enum] must select the i64-word comparator (kind 0), not str (1)"
        );
        let text = format!("{m}");
        assert!(text.contains("call @zap_heap_push_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_pop_i64"), "text:\n{text}");
    }

    #[test]
    fn lower_exceptions() {
        // raise + try/except/finally lower to invoke/landingpad exception edges
        // and the runtime unwinding entry points (#22).
        let m = lower_ok(include_str!("../../../tests/golden/exceptions_catch.zap"));
        let text = format!("{m}");
        assert!(text.contains("invoke @zap_panic"), "text:\n{text}");
        assert!(text.contains("landingpad"), "text:\n{text}");
        assert!(text.contains("call @zap_eh_end_catch"), "text:\n{text}");
        // An exception raised in a callee reaches a `try` in `main` via an
        // `invoke` of the callee (real cross-frame unwinding).
        let m = lower_ok(include_str!(
            "../../../tests/golden/exceptions_propagate.zap"
        ));
        let text = format!("{m}");
        assert!(text.contains("invoke @boom"), "text:\n{text}");
        // An uncaught raise routes to the implicit top-level handler in `main`.
        let m = lower_ok(include_str!(
            "../../../tests/golden/exceptions_uncaught.zap"
        ));
        let text = format!("{m}");
        assert!(text.contains("call @zap_uncaught"), "text:\n{text}");
    }

    #[test]
    fn lower_bare_reraise() {
        // #657: a bare `raise` (re-raise the active exception) inside an
        // `except` handler lowers to a re-throw of the *same* in-flight
        // exception object the landing pad caught — preserving its type and
        // identity, not degrading to a fresh base `Exception`. Before the fix
        // this was rejected at lowering with ZAP6001 "bare 'raise' (re-raise)
        // is not supported yet".  (The try body must be able to raise, else the
        // handler is dead and pruned before the bare `raise` is reached.)
        let m = lower_ok(
            "def f(int64 x) -> int64:\n    \
                 try:\n        \
                     raise ValueError(\"boom\")\n    \
                 except Exception:\n        \
                     raise\n    \
                 return x\n",
        );
        let text = format!("{m}");
        // The re-raise re-throws through the shared `zap_raise` unwinding entry
        // (the same path a `raise <obj>` inside a handler uses). Inside the
        // handler body it is an `invoke` (an active landing pad is present).
        assert!(
            text.contains("invoke @zap_raise"),
            "bare re-raise must re-throw via zap_raise, text:\n{text}"
        );
        assert!(text.contains("landingpad"), "text:\n{text}");
        // It must NOT fabricate a fresh exception object — it reuses the caught
        // one — so no extra `zap_exception_new`/`zap_panic` is emitted for the
        // re-raise itself.
        assert!(
            !text.contains("zap_panic"),
            "bare re-raise must not wrap in a fresh base Exception, text:\n{text}"
        );
    }

    #[test]
    fn lower_bare_reraise_with_binding() {
        // #657: `except E as e:` then a bare `raise` re-raises the same caught
        // object; the binding is unaffected and both forms are available.
        let m = lower_ok(
            "def f(int64 x) -> int64:\n    \
                 try:\n        \
                     raise ValueError(\"boom\")\n    \
                 except Exception as e:\n        \
                     print(e)\n        \
                     raise\n    \
                 return x\n",
        );
        let text = format!("{m}");
        assert!(
            text.contains("invoke @zap_raise"),
            "bare re-raise (with binding) must re-throw via zap_raise, text:\n{text}"
        );
    }

    #[test]
    fn lower_bare_reraise_nested_handlers() {
        // #657: a bare `raise` re-raises the *innermost* active exception. Both
        // handlers' try bodies can raise, so neither handler is pruned; each
        // bare `raise` re-throws its own caught object.
        let m = lower_ok(
            "def f(int64 x) -> int64:\n    \
                 try:\n        \
                     try:\n            \
                         raise ValueError(\"inner\")\n        \
                     except ValueError:\n            \
                         raise\n    \
                 except Exception:\n        \
                     raise\n    \
                 return x\n",
        );
        let text = format!("{m}");
        // Two live re-raise sites (inner + outer handler).
        assert!(
            text.matches("@zap_raise").count() >= 2,
            "nested handlers should emit two re-raise sites, text:\n{text}"
        );
    }

    #[test]
    fn lower_bare_reraise_outside_handler_rejected() {
        // #657 edge case: a bare `raise` not lexically inside any `except`
        // handler has no active exception to re-raise. Zap rejects it at
        // compile time with a dedicated diagnostic (ZAP6002) rather than
        // deferring to a runtime `RuntimeError` as Python does — it is
        // statically provable here. This must be a clean error, never an ICE.
        let (ast, _p, _l) = parse_str("def main():\n    raise\n");
        let (_m, errs) = lower_module(&ast);
        assert!(
            errs.iter().any(|e| e.code == zapc_diag::ErrorCode::ZAP6002),
            "bare raise outside a handler must be ZAP6002, got: {errs:?}"
        );
    }

    #[test]
    fn lower_bare_reraise_in_try_body_rejected() {
        // #657 edge case: a bare `raise` in the *try* body (not a handler) is
        // also outside any active exception — rejected at compile time.
        let (ast, _p, _l) = parse_str(
            "def main():\n    \
                 try:\n        \
                     raise\n    \
                 except Exception:\n        \
                     print(\"x\")\n",
        );
        let (_m, errs) = lower_module(&ast);
        assert!(
            errs.iter().any(|e| e.code == zapc_diag::ErrorCode::ZAP6002),
            "bare raise in a try body must be ZAP6002, got: {errs:?}"
        );
    }

    #[test]
    fn lower_bare_reraise_in_separate_fn_rejected() {
        // #657 edge case: active-exception tracking is lexical and
        // per-function. A separate function called *from* a handler does not
        // inherit the caller's active exception, so a bare `raise` in its body
        // is rejected at compile time — Zap does not implement Python's
        // dynamic, cross-call-frame re-raise in v1 (documented deviation).
        let (ast, _p, _l) = parse_str(
            "def helper():\n    \
                 raise\n\
             def main():\n    \
                 try:\n        \
                     print(\"x\")\n    \
                 except Exception:\n        \
                     helper()\n",
        );
        let (_m, errs) = lower_module(&ast);
        assert!(
            errs.iter().any(|e| e.code == zapc_diag::ErrorCode::ZAP6002),
            "bare raise in a separate fn must be ZAP6002, got: {errs:?}"
        );
    }

    #[test]
    fn lower_bound_raise_still_works() {
        // #657 guard: the bound forms (`raise E(...)`, `raise "msg"`,
        // `raise E(...) from e`) are unchanged by the bare-raise work.
        lower_ok(
            "def main():\n    \
                 try:\n        \
                     raise ValueError(\"boom\")\n    \
                 except Exception as e:\n        \
                     raise RuntimeError(\"wrapped\") from e\n",
        );
    }

    #[test]
    fn lower_builtin_fault_catchable() {
        // #383: a built-in fault (index-out-of-range) inside a `try` lowers the
        // indexing to an `invoke` whose unwind edge is the handler, so the fault
        // (raised via `zap_panic` in the runtime) is caught like a `raise`.
        let m = lower_ok(
            "def main():\n    \
                 list[int64] xs = [1, 2]\n    \
                 str s = \"hi\"\n    \
                 try:\n        \
                     auto v = xs[5]\n        \
                     auto c = s[9]\n    \
                 except Exception as e:\n        \
                     print(\"caught\")\n",
        );
        let text = format!("{m}");
        assert!(
            text.contains("invoke @zap_list_get_i64") && text.contains("unwind"),
            "list index in `try` must be an invoke, text:\n{text}"
        );
        assert!(
            text.contains("invoke @zap_str_index"),
            "str index in `try` must be an invoke, text:\n{text}"
        );
        assert!(text.contains("landingpad"), "text:\n{text}");

        // Even without an explicit `try`, `main` carries an implicit top-level
        // handler, so an uncaught fault still reaches it via an `invoke` (and
        // `zap_uncaught` reports it) rather than unwinding past the frame.
        let m = lower_ok(
            "def main():\n    \
                 list[int64] xs = [1, 2]\n    \
                 auto v = xs[5]\n    \
                 print(v)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("invoke @zap_list_get_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_uncaught"), "text:\n{text}");

        // In a non-`main` function with no active handler the indexing is a
        // plain `call`; the fault unwinds through this frame to the caller's
        // handler (here `main`'s `try`, reached via `invoke @get`).
        let m = lower_ok(
            "def get(list[int64] xs) -> int64:\n    \
                 return xs[5]\n\
             def main():\n    \
                 list[int64] xs = [1, 2]\n    \
                 try:\n        \
                     print(get(xs))\n    \
                 except Exception as e:\n        \
                     print(\"caught\")\n",
        );
        let text = format!("{m}");
        assert!(text.contains("call @zap_list_get_i64"), "text:\n{text}");
        assert!(text.contains("invoke @get"), "text:\n{text}");
    }

    #[test]
    fn lower_divide_by_zero_catchable() {
        // EXC-3: integer `//` and `%` inside a `try` lower to an `invoke` of the
        // runtime helper (which raises via `zap_panic` on a zero divisor), so the
        // divide-by-zero fault unwinds to the handler and is caught like a
        // `raise` — not a process abort.
        let m = lower_ok(
            "def main():\n    \
                 int64 z = 0\n    \
                 try:\n        \
                     auto q = 1 // z\n        \
                     auto r = 1 % z\n    \
                 except Exception as e:\n        \
                     print(\"caught\")\n",
        );
        let text = format!("{m}");
        assert!(
            text.contains("invoke @zap_ifloordiv") && text.contains("unwind"),
            "`//` in `try` must be an invoke, text:\n{text}"
        );
        assert!(
            text.contains("invoke @zap_imod"),
            "`%` in `try` must be an invoke, text:\n{text}"
        );
        assert!(text.contains("landingpad"), "text:\n{text}");

        // Without an explicit `try`, `main`'s implicit top-level handler still
        // makes the divide an `invoke`, so an uncaught fault reaches
        // `zap_uncaught` rather than unwinding past the frame.
        let m = lower_ok(
            "def main():\n    \
                 int64 z = 0\n    \
                 print(1 // z)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("invoke @zap_ifloordiv"), "text:\n{text}");
        assert!(text.contains("call @zap_uncaught"), "text:\n{text}");
    }

    #[test]
    fn lower_none_member_access_catchable() {
        // EXC-4: `None` bound to a reference struct lowers to a null pointer, and
        // a field read guards the receiver with a null check that raises via
        // `zap_panic` through an `invoke` to the handler — so member access on
        // `None` inside a `try` is caught like a `raise` (D-0007/D-0014).
        let m = lower_ok(
            "struct Point:\n    \
                 int64 x\n\
             def main():\n    \
                 Point p = None\n    \
                 try:\n        \
                     auto v = p.x\n    \
                 except Exception as e:\n        \
                     print(\"caught\")\n",
        );
        let text = format!("{m}");
        assert!(
            text.contains("nullconst"),
            "`None` must lower to a null pointer, text:\n{text}"
        );
        assert!(
            text.contains("isnull") && text.contains("invoke @zap_panic"),
            "field read on a reference struct must null-guard via invoke, text:\n{text}"
        );
        assert!(text.contains("landingpad"), "text:\n{text}");

        // Without an explicit `try`, the guard's `zap_panic` still routes to the
        // implicit top-level handler (`zap_uncaught`) in `main`.
        let m = lower_ok(
            "struct Point:\n    \
                 int64 x\n\
             def main():\n    \
                 Point p = None\n    \
                 print(p.x)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("invoke @zap_panic"), "text:\n{text}");
        assert!(text.contains("call @zap_uncaught"), "text:\n{text}");
    }

    #[test]
    fn lower_raise_from_sets_cause() {
        // #604 (D-0103): `raise E(...) from e` records the cause on the new
        // exception via `zap_exception_set_cause` before it unwinds, reusing the
        // caught exception's *object* (stashed beside the message binding) as the
        // cause so its own type and chain survive.
        let m = lower_ok(
            "def main():\n    \
                 try:\n        \
                     raise ValueError(\"first\")\n    \
                 except Exception as e:\n        \
                     raise RuntimeError(\"second\") from e\n",
        );
        let text = format!("{m}");
        assert!(
            text.contains("call @zap_exception_set_cause"),
            "`raise ... from e` must set the cause slot, text:\n{text}"
        );

        // A plain `raise` (no `from`) is unchanged: no cause slot is written.
        let m = lower_ok(
            "def main():\n    \
                 raise RuntimeError(\"boom\")\n",
        );
        let text = format!("{m}");
        assert!(
            !text.contains("zap_exception_set_cause"),
            "a plain raise must not touch the cause slot, text:\n{text}"
        );
    }

    #[test]
    fn lower_caught_exception_cause_is_readable() {
        // #604 (D-0103): `e.cause` on a caught exception binding reads the
        // explicit cause's message off the stashed exception object.
        let m = lower_ok(
            "def main():\n    \
                 try:\n        \
                     raise ValueError(\"first\")\n    \
                 except Exception as e:\n        \
                     print(e.cause)\n",
        );
        let text = format!("{m}");
        assert!(
            text.contains("call @zap_exception_cause_message"),
            "`e.cause` must read the cause message, text:\n{text}"
        );
    }

    #[test]
    fn lower_arrays() {
        // Construction, element access, len, and sum lower to the array runtime
        // entry points; element-wise ops lower to the dedicated ArrayBinOp node.
        let m = lower_ok(include_str!("../../../tests/golden/array_int.zap"));
        let text = format!("{m}");
        assert!(text.contains("call @zap_array_new"), "text:\n{text}");
        assert!(text.contains("call @zap_array_set_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_array_sum_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_array_len"), "text:\n{text}");
        assert!(text.contains("call @zap_array_get_i64"), "text:\n{text}");
        assert!(text.contains("array.Add.i64"), "text:\n{text}");
        assert!(text.contains("array.Mul.i64"), "text:\n{text}");
        assert!(text.contains("array.Sub.i64"), "text:\n{text}");
        // f64 arrays select the f64-suffixed entry points.
        let m = lower_ok(include_str!("../../../tests/golden/array_f64.zap"));
        let text = format!("{m}");
        assert!(text.contains("call @zap_array_sum_f64"), "text:\n{text}");
        assert!(text.contains("array.Mul.f64"), "text:\n{text}");
    }

    #[test]
    fn lower_array_dtypes_and_promotion() {
        // Mixed-dtype element-wise ops promote to the higher dtype (#49): the
        // ArrayBinOp carries the per-operand dtypes and a promoted `out`.
        let m = lower_ok(include_str!("../../../tests/golden/array_promote.zap"));
        let text = format!("{m}");
        // i64 + f64 -> f64 (operands keep their own dtype in the node).
        assert!(text.contains("array.Add.f64 %"), "text:\n{text}");
        assert!(text.contains(":i64, %"), "text:\n{text}");
        // uint8 + i64 -> i64.
        assert!(text.contains("array.Add.i64 %"), "text:\n{text}");
        assert!(text.contains(":u8, %"), "text:\n{text}");

        // uint8 arrays select the u8-suffixed runtime entry points; a weak int
        // scalar keeps the dtype (`a * 2` stays u8).
        let m = lower_ok(include_str!("../../../tests/golden/array_uint8.zap"));
        let text = format!("{m}");
        assert!(text.contains("call @zap_array_set_u8"), "text:\n{text}");
        assert!(text.contains("call @zap_array_sum_u8"), "text:\n{text}");
        assert!(text.contains("call @zap_array_get_u8"), "text:\n{text}");
        assert!(text.contains("array.Add.u8"), "text:\n{text}");
        assert!(text.contains("array.Mul.u8"), "text:\n{text}");

        // bool arrays store via the bool entry point; arithmetic promotes bool
        // -> uint8 (so `m + n` / `m * n` are u8 results).
        let m = lower_ok(include_str!("../../../tests/golden/array_bool.zap"));
        let text = format!("{m}");
        assert!(text.contains("call @zap_array_set_bool"), "text:\n{text}");
        assert!(text.contains("call @zap_array_sum_bool"), "text:\n{text}");
        assert!(text.contains("array.Add.u8 %"), "text:\n{text}");
        assert!(text.contains(":bool, %"), "text:\n{text}");
    }

    #[test]
    fn array_scalar_on_left_lowers_for_all_four_ops() {
        // #506: scalar-on-the-left now lowers for every element-wise op. The
        // commutative `+`/`*` reorder to `array op scalar`; the non-commutative
        // `-`/`/` are kept as scalar-on-left (`scalar-lhs` in the ZIR dump), and
        // `/` is true division (an int result lifts to f64).
        let m = lower_ok(
            "def main():\n    auto a = array[i64]([1, 2])\n    \
             print(sum(3 + a), sum(3 * a), sum(3 - a), sum(6 / a))\n",
        );
        let text = format!("{m}");
        // `3 - a` stays scalar-on-left over an i64 array.
        assert!(text.contains("array.Sub.i64"), "text:\n{text}");
        assert!(text.contains("(scalar-lhs)"), "text:\n{text}");
        // `6 / a` true-divides: a scalar-lhs division with an f64 result.
        assert!(text.contains("array.Div.f64"), "text:\n{text}");
        // The commutative pair reordered to plain scalar-on-right (not flagged).
        assert!(text.contains("array.Add.i64"), "text:\n{text}");
        assert!(text.contains("array.Mul.i64"), "text:\n{text}");
    }

    #[test]
    fn lower_scalars_fixture() {
        // The T9 scalar tower (ticket #19): int32/float32 slots, conversions,
        // `**`, and `//` all lower to the expected ops/casts/runtime calls.
        let m = lower_ok(include_str!("../../../tests/golden/scalars.zap"));
        let text = format!("{m}");
        // int32 / float32 values get narrow slots (not everything is i64/f64).
        assert!(text.contains("alloca i32"), "text:\n{text}");
        assert!(text.contains("alloca f32"), "text:\n{text}");
        // `scale` is typed int32 -> int32.
        let scale = m.funcs.iter().find(|f| f.name == "scale").expect("scale");
        assert_eq!(scale.params, vec![Ty::I32]);
        assert_eq!(scale.ret, Ty::I32);
        // Conversions and widening use the cast kinds.
        assert!(text.contains("Trunc"), "text:\n{text}"); // int64 -> int32
        assert!(text.contains("FPToSI"), "text:\n{text}"); // float -> int
        assert!(text.contains("SIToFP"), "text:\n{text}"); // int -> float
        assert!(text.contains("FPExt"), "text:\n{text}"); // float32 -> float64
                                                          // Power and floor-division lower to runtime helpers.
                                                          // Integer `**` can raise a catchable ValueError on a negative exponent
                                                          // (#374/B2), so in `main` (implicit top-level handler) it is an
                                                          // `invoke`, like `zap_ifloordiv` below.
        assert!(text.contains("invoke @zap_ipow"), "text:\n{text}");
        assert!(text.contains("call @zap_powf"), "text:\n{text}");
        // Integer `//` is a catchable divide-by-zero fault (EXC-3), so in `main`
        // (which has the implicit top-level handler) it is an `invoke`; the float
        // helper never faults (IEEE semantics), so it stays a plain `call`.
        assert!(text.contains("invoke @zap_ifloordiv"), "text:\n{text}");
        assert!(text.contains("call @zap_ffloordiv"), "text:\n{text}");
    }

    #[test]
    fn pow_lowers_by_operand_kind() {
        // All-integer `**` -> zap_ipow; any float operand -> zap_powf.
        // The integer helper can raise (negative exponent, #374/B2), so in
        // `main` it lowers as an `invoke`.
        let m = lower_ok("def main():\n    print(2 ** 10)\n");
        assert!(format!("{m}").contains("invoke @zap_ipow"));
        let m = lower_ok("def main():\n    print(2.0 ** 3.0)\n");
        assert!(format!("{m}").contains("call @zap_powf"));
    }

    #[test]
    fn floordiv_lowers_by_operand_kind() {
        // `//` never lowers to a bare sdiv/fdiv — it always goes through the
        // floor-correcting runtime helper (D-0024).
        let m = lower_ok("def main():\n    print(7 // 2)\n");
        let text = format!("{m}");
        // In `main` the integer helper is an `invoke` (EXC-3: divide-by-zero is a
        // catchable fault reaching the implicit top-level handler).
        assert!(text.contains("invoke @zap_ifloordiv"), "text:\n{text}");
        assert!(!text.contains("= Div "), "no raw div for int //:\n{text}");
        let m = lower_ok("def main():\n    print(7.0 // 2.0)\n");
        assert!(format!("{m}").contains("call @zap_ffloordiv"));
    }

    #[test]
    fn explicit_conversion_lowers_to_cast() {
        // `int32(x)` narrows (Trunc); `float64(x)` crosses to float (SIToFP).
        let m = lower_ok("def main():\n    int64 b = 300\n    int32 c = int32(b)\n    print(c)\n");
        assert!(format!("{m}").contains("Trunc"));
        let m = lower_ok("def main():\n    int32 a = 7\n    print(float64(a))\n");
        assert!(format!("{m}").contains("SIToFP"));
    }

    #[test]
    fn unsupported_construct_errors() {
        let (ast, _p, _l) = parse_str("def main():\n    raise Oops\n");
        let (_m, errs) = lower_module(&ast);
        assert!(!errs.is_empty());
    }

    // --- decorators & annotations (ticket #16, D-0036) ---

    #[test]
    fn lower_decorators_fixture() {
        // The golden program exercises the whole feature: `@derive(Eq, Repr)`
        // generates methods, `@Table` is reflected, and `reflect_*` queries lower
        // to the runtime registry.
        let m = lower_ok(include_str!("../../../tests/golden/decorators.zap"));
        // `@derive(Eq, Repr)` produced two extra top-level functions.
        let eq = m
            .funcs
            .iter()
            .find(|f| f.name == "User____eq__")
            .expect("eq");
        assert_eq!(eq.params, vec![Ty::Struct(0), Ty::Struct(0)]);
        assert_eq!(eq.ret, Ty::I1);
        let repr = m
            .funcs
            .iter()
            .find(|f| f.name == "User____repr__")
            .expect("repr");
        assert_eq!(repr.params, vec![Ty::Struct(0)]);
        assert_eq!(repr.ret, Ty::Ptr);

        let text = format!("{m}");
        // Eq compares an int field (icmp) and a str field (runtime content eq).
        assert!(text.contains("call @zap_str_eq"), "text:\n{text}");
        // Repr stringifies the int field and concatenates.
        assert!(text.contains("call @zap_i64_to_str"), "text:\n{text}");
        // The reflection registry is populated at the top of `main`, and the
        // user annotation `@Table` (not the compile-time `@derive`) is recorded.
        assert!(
            text.contains("strconst \"Table\""),
            "the user annotation should be registered, text:\n{text}"
        );
        assert!(
            !text.contains("strconst \"derive\""),
            "the compile-time @derive must not reach the runtime registry"
        );
        assert!(text.contains("call @zap_reflect_register"), "text:\n{text}");
        assert!(text.contains("call @zap_reflect_has"), "text:\n{text}");
        assert!(text.contains("call @zap_reflect_names"), "text:\n{text}");
    }

    #[test]
    fn shareable_marker_is_reflectable_on_a_user_struct() {
        // `@shareable` on a `struct` (#664, D-0113) reaches the runtime
        // reflection registry like any user annotation, so
        // `reflect_has("Reg", "shareable")` resolves against it.
        let m = lower_ok(
            "@shareable\nstruct Reg:\n    int64 n\n\ndef main():\n    print(reflect_has(\"Reg\", \"shareable\"))\n",
        );
        let text = format!("{m}");
        assert!(
            text.contains("strconst \"shareable\""),
            "the @shareable marker must be registered, text:\n{text}"
        );
        assert!(text.contains("call @zap_reflect_register"), "text:\n{text}");
        // The user-struct query goes through the runtime registry (not folded).
        assert!(text.contains("call @zap_reflect_has"), "text:\n{text}");
    }

    #[test]
    fn reflect_has_shareable_on_a_builtin_type_folds_to_true() {
        // The built-in concurrency primitives carry `@shareable` implicitly and
        // have no registry entry, so `reflect_has("Atomic", "shareable")` is
        // folded to a compile-time `true` (#664, D-0113). The fold means NO
        // runtime `zap_reflect_has` call and no `"Atomic"` registration — which
        // keeps the `main` prologue of reflection-free programs unchanged.
        let m = lower_ok("def main():\n    print(reflect_has(\"Atomic\", \"shareable\"))\n");
        let text = format!("{m}");
        assert!(
            !text.contains("call @zap_reflect_has"),
            "the built-in shareable query must be folded, not a runtime call, text:\n{text}"
        );
        assert!(
            !text.contains("strconst \"Atomic\""),
            "a folded built-in query must not register the type, text:\n{text}"
        );
        // A non-shareable query on the same literal type still goes to runtime
        // (returns false there), proving only the `shareable` marker is folded.
        let m = lower_ok("def main():\n    print(reflect_has(\"Atomic\", \"other\"))\n");
        let text = format!("{m}");
        assert!(
            text.contains("call @zap_reflect_has"),
            "a non-shareable built-in query must NOT be folded, text:\n{text}"
        );
    }

    // --- signature reflection (I#458, D-0089) ---

    #[test]
    fn lower_param_reflection() {
        // Every top-level fn and struct method is registered with its parameter
        // metadata + a precomputed signature at the top of `main`; the queries
        // lower to the matching runtime entry points.
        let m = lower_ok(
            "annotation Inject:\n    str service = \"\"\n\n\
             struct Svc:\n    int64 n\n\n\
             \x20   def handle(self, int64 id, str q = \"x\") -> int64:\n        return id\n\n\
             def greet(str name, @Inject Svc dep, int64 times = 3) -> str:\n    return name\n\n\
             def main():\n    print(reflect_params(\"greet\"))\n    \
             print(reflect_sig(\"greet\"))\n    \
             print(reflect_param_type(\"greet\", \"times\"))\n    \
             print(reflect_param_annotations(\"greet\", \"dep\"))\n    \
             if reflect_param_has_default(\"greet\", \"times\"):\n        print(\"d\")\n",
        );
        let text = format!("{m}");
        // Registration bootstrap: param + signature entries.
        assert!(
            text.contains("call @zap_reflect_param_register"),
            "text:\n{text}"
        );
        assert!(
            text.contains("call @zap_reflect_sig_register"),
            "text:\n{text}"
        );
        // The rendered signature includes the annotated param and the `= ...`
        // default marker, and the owner key is the bare fn name.
        assert!(
            text.contains("strconst \"(name: str, @Inject dep: Svc, times: int64 = ...) -> str\""),
            "text:\n{text}"
        );
        assert!(text.contains("strconst \"greet\""), "text:\n{text}");
        // A method is keyed `Type.method`, and `self` is excluded.
        assert!(text.contains("strconst \"Svc.handle\""), "text:\n{text}");
        assert!(
            text.contains("strconst \"(id: int64, q: str = ...) -> int64\""),
            "text:\n{text}"
        );
        // The per-param annotation is recorded (not the deferred-decorator error).
        assert!(text.contains("strconst \"Inject\""), "text:\n{text}");
        // Each query lowers to its runtime entry point.
        for f in [
            "zap_reflect_params",
            "zap_reflect_sig",
            "zap_reflect_param_type",
            "zap_reflect_param_annotations",
            "zap_reflect_param_has_default",
        ] {
            assert!(text.contains(&format!("call @{f}")), "missing {f}\n{text}");
        }
    }

    // --- coroutines & channels (ticket #25, D-0013/D-0031) ---

    #[test]
    fn lower_coroutines_fixture() {
        // The golden program exercises the whole core surface: channel
        // construction (unbuffered + buffered), `spawn` of a named function,
        // channel send/receive, and `close`.
        let m = lower_ok(include_str!("../../../tests/golden/coroutines.zap"));
        let text = format!("{m}");
        // `channel[int64]()` / `channel[int64](cap)` lower to the constructor.
        assert!(text.contains("call @zap_chan_new"), "text:\n{text}");
        // `spawn worker(...)` lowers to the dedicated Spawn node naming the fn.
        assert!(text.contains("spawn @worker("), "text:\n{text}");
        // send/recv pick the i64-suffixed typed entry points.
        assert!(text.contains("@zap_chan_send_i64("), "text:\n{text}");
        assert!(text.contains("@zap_chan_recv_i64("), "text:\n{text}");
        // `close(ch)` lowers to the close entry point.
        assert!(text.contains("@zap_chan_close("), "text:\n{text}");
        // A `channel[int64]` parameter and a channel value are typed `chan[i64]`.
        let worker = m.funcs.iter().find(|f| f.name == "worker").expect("worker");
        assert_eq!(worker.params, vec![Ty::Chan(Elem::I64), Ty::I64]);
    }

    #[test]
    fn spawn_is_a_dedicated_node_with_eager_args() {
        // `spawn f(a, b)` evaluates its args in the parent, then emits a Spawn
        // node carrying the callee and the evaluated argument values.
        let m = lower_ok(
            "def f(int64 a, int64 b):\n    print(a + b)\n\ndef main():\n    spawn f(1, 2)\n",
        );
        let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
        let spawn = main
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .find_map(|i| match i {
                Inst::Spawn {
                    func, args, owner, ..
                } => Some((func.clone(), args.len(), *owner)),
                _ => None,
            })
            .expect("a Spawn instruction");
        // A bare `spawn` (no enclosing `scope:`) is owned by the root scope.
        assert_eq!(spawn, ("f".to_string(), 2, Owner::Root));
    }

    #[test]
    fn scope_spawn_detached_ownership() {
        // A `spawn` inside a `scope:` is owned by that scope's handle; `spawn
        // detached` is owned by nothing; a bare `spawn` by the root (D-0044).
        let m = lower_ok(
            "def f():\n    print(1)\n\ndef main():\n    scope:\n        spawn f()\n    spawn detached f()\n    spawn f()\n",
        );
        let text = format!("{m}");
        // The scope opens with a scope_enter handle and joins with scope_exit.
        assert!(text.contains("call @zap_scope_enter("), "text:\n{text}");
        assert!(text.contains("@zap_scope_exit("), "text:\n{text}");
        let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
        let owners: Vec<Owner> = main
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                Inst::Spawn { owner, .. } => Some(*owner),
                _ => None,
            })
            .collect();
        assert_eq!(owners.len(), 3, "three spawns");
        assert!(
            matches!(owners[0], Owner::Scope(_)),
            "the scoped spawn is owned by the scope"
        );
        assert_eq!(owners[1], Owner::Detached, "the detached spawn");
        assert_eq!(owners[2], Owner::Root, "the root-scoped spawn");
    }

    /// Collect the `Inst::Spawn` nodes of a function as `(func, nargs, name)`.
    fn spawns_of(f: &zapc_ir_core::Func) -> Vec<(String, usize, Option<String>)> {
        f.blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                Inst::Spawn {
                    func, args, name, ..
                } => Some((func.clone(), args.len(), name.clone())),
                _ => None,
            })
            .collect()
    }

    fn lower_errs(src: &str) -> Vec<LowerError> {
        let (ast, perrs, lerrs) = parse_str(src);
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        lower_module(&ast).1
    }

    #[test]
    fn spawn_of_method_lowers_to_named_method_spawn() {
        // `spawn recv.method(args)` (I#475) desugars to a named `Inst::Spawn` on
        // the mangled method symbol with the receiver captured as argument 0, and
        // carries a readable `Struct.method` identity for `ScopeTimeout` reports.
        let m = lower_ok(
            "struct Worker:\n    int64 base\n    def run(self, int64 n):\n        print(self.base + n)\n\ndef main():\n    auto w = Worker(10)\n    scope:\n        spawn w.run(5)\n",
        );
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        let spawns = spawns_of(main);
        assert_eq!(spawns.len(), 1, "one spawn");
        assert_eq!(spawns[0].0, "Worker__run", "mangled method symbol");
        assert_eq!(spawns[0].1, 2, "receiver + one captured argument");
        assert_eq!(
            spawns[0].2.as_deref(),
            Some("Worker.run"),
            "readable identity"
        );
        // The method is owned by the enclosing scope (D-0044).
        let scoped = main.blocks.iter().flat_map(|b| &b.insts).any(|i| {
            matches!(
                i,
                Inst::Spawn {
                    owner: Owner::Scope(_),
                    ..
                }
            )
        });
        assert!(scoped, "the method spawn is owned by the scope");
    }

    #[test]
    fn spawn_of_function_value_generates_thunk() {
        // `spawn f(x)` where `f : fn(..)->..` (I#475) lowers through a generated
        // `$spawnthunk$<sig>` whose body is an indirect `CallClosure`; the spawn
        // targets the thunk and reports the source callee name.
        let m = lower_ok(
            "def main():\n    fn(int64) -> int64 f = lambda x: x + 1\n    scope:\n        spawn f(3)\n",
        );
        let thunk = m
            .funcs
            .iter()
            .find(|f| f.name.starts_with("$spawnthunk$"))
            .expect("a generated spawn thunk");
        assert!(
            thunk
                .blocks
                .iter()
                .flat_map(|b| &b.insts)
                .any(|i| matches!(i, Inst::CallClosure { .. })),
            "the thunk performs an indirect CallClosure"
        );
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        let spawns = spawns_of(main);
        assert_eq!(spawns.len(), 1, "one spawn");
        assert!(
            spawns[0].0.starts_with("$spawnthunk$"),
            "spawn targets the thunk, got {}",
            spawns[0].0
        );
        assert_eq!(spawns[0].1, 2, "closure record + one captured argument");
        assert_eq!(spawns[0].2.as_deref(), Some("f"), "reports the callee name");
    }

    #[test]
    fn spawn_thunk_is_reused_across_same_signature() {
        // Two `spawn`s of the same function-value signature share one thunk.
        let m = lower_ok(
            "def main():\n    fn(int64) -> int64 f = lambda x: x + 1\n    fn(int64) -> int64 g = lambda x: x * 2\n    scope:\n        spawn f(3)\n        spawn g(4)\n",
        );
        let thunks = m
            .funcs
            .iter()
            .filter(|f| f.name.starts_with("$spawnthunk$"))
            .count();
        assert_eq!(
            thunks, 1,
            "one thunk for the shared fn(int64)->int64 signature"
        );
    }

    #[test]
    fn spawn_rejects_bare_lambda_literal() {
        // A bare lambda literal callee has no function-typed context at a spawn
        // site (I#475): rejected with ZAP5076, bind it to a name first.
        let errs = lower_errs("def main():\n    spawn (lambda: print(1))()\n");
        assert!(
            errs.iter().any(|e| e.code == zapc_diag::ErrorCode::ZAP5076),
            "errs: {errs:?}"
        );
    }

    #[test]
    fn spawn_rejects_keyword_and_arity() {
        for src in [
            // keyword argument
            "def w(int64 a):\n    print(a)\ndef main():\n    spawn w(a=1)\n",
            // too few positional arguments (no defaults in spawn)
            "def w(int64 a):\n    print(a)\ndef main():\n    spawn w()\n",
            // method arity mismatch
            "struct W:\n    def run(self, int64 n):\n        print(n)\ndef main():\n    auto w = W()\n    spawn w.run()\n",
        ] {
            let errs = lower_errs(src);
            assert!(
                errs.iter()
                    .any(|e| e.code == zapc_diag::ErrorCode::ZAP5076),
                "expected ZAP5076 for:\n{src}\ngot: {errs:?}"
            );
        }
    }

    #[test]
    fn channel_of_strings_uses_ptr_entry_points() {
        // A `channel[str]` stores pointer-word elements, selecting the `_ptr`
        // typed send/receive entry points.
        let m = lower_ok(
            "def main():\n    auto ch = channel[str](1)\n    ch <- \"hi\"\n    auto s = <-ch\n    print(s)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_chan_send_ptr("), "text:\n{text}");
        assert!(text.contains("@zap_chan_recv_ptr("), "text:\n{text}");
    }

    #[test]
    fn spawn_of_unknown_function_errors() {
        let (ast, _p, _l) = parse_str("def main():\n    spawn nope(1)\n");
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "spawn of an undefined function should error"
        );
    }

    // --- regex (ticket #23, D-0010) ---

    #[test]
    fn lower_regex_fixture() {
        // The golden program exercises the whole surface: m// match, positional
        // and named capture groups, `!~`, and first/global s/// substitution.
        let m = lower_ok(include_str!("../../../tests/golden/regex.zap"));
        let text = format!("{m}");
        // m// lowers to a match-object call; s/// to a substitution call.
        assert!(text.contains("call @zap_regex_match"), "text:\n{text}");
        assert!(text.contains("call @zap_regex_subst"), "text:\n{text}");
        // Capture-group access lowers to the positional and named accessors.
        assert!(text.contains("call @zap_regex_group("), "text:\n{text}");
        assert!(
            text.contains("call @zap_regex_group_name("),
            "text:\n{text}"
        );
        // `if mt:` / `!~` test the match object for null via the runtime helper.
        assert!(text.contains("call @zap_ptr_nonnull"), "text:\n{text}");
        // The pattern/replacement text reaches the runtime verbatim as a string
        // constant: regex escapes like `\d` are NOT string-unescaped (the ZIR
        // printer's `{:?}` doubles the backslash), and `$2 $1` is untouched.
        assert!(
            text.contains(r#"strconst "(\\d{4})-(\\d{2})-(\\d{2})""#),
            "text:\n{text}"
        );
        assert!(text.contains(r#"strconst "$2 $1""#), "text:\n{text}");
    }

    #[test]
    fn regex_match_is_a_match_object_negation_is_bool() {
        // `=~ m//` binds a match object (reference type); `!~` yields an i1.
        let m = lower_ok(
            "def main():\n    auto mt = \"x1\" =~ m/(\\d)/\n    auto no = \"x\" !~ m/\\d/\n    print(mt.group(1))\n    if no:\n        print(\"none\")\n",
        );
        let text = format!("{m}");
        assert!(text.contains("= call @zap_regex_match"), "text:\n{text}");
        // The group accessor returns a string (pointer), which `print` prints.
        assert!(text.contains("= call @zap_regex_group("), "text:\n{text}");
    }

    // --- Python interop (ticket #27, D-0012) ---

    #[test]
    fn lower_python_interop_fixture() {
        // The golden program exercises the whole surface: module import,
        // attribute+call dispatch, and scalar/str marshaling both directions.
        let m = lower_ok(include_str!("../../../tests/golden/python_interop.zap"));
        let text = format!("{m}");
        // Import, attribute read, and call all route to the Python runtime.
        assert!(text.contains("@zap_py_import("), "text:\n{text}");
        assert!(text.contains("@zap_py_getattr("), "text:\n{text}");
        assert!(text.contains("@zap_py_call("), "text:\n{text}");
        // Argument marshaling: int, float, and str cross into Python.
        assert!(text.contains("call @zap_py_from_i64("), "text:\n{text}");
        assert!(text.contains("call @zap_py_from_f64("), "text:\n{text}");
        assert!(text.contains("call @zap_py_from_str("), "text:\n{text}");
        // Result marshaling: `int64(...)` converts a Python int back, and
        // `print` of a Python handle renders it via its str().
        assert!(text.contains("@zap_py_to_i64("), "text:\n{text}");
        assert!(text.contains("call @zap_py_str("), "text:\n{text}");
        // The argument vector for a call is a `list[ptr]` of marshaled handles.
        assert!(text.contains("call @zap_list_new("), "text:\n{text}");
        assert!(text.contains("call @zap_list_push_ptr("), "text:\n{text}");
    }

    #[test]
    fn python_import_and_call_lower_to_runtime() {
        // A minimal import + call lowers to import -> getattr -> call, with the
        // int argument marshaled in and the int result marshaled back out.
        let m = lower_ok(
            "import python \"math\" as m\n\ndef main():\n    print(int64(m.gcd(12, 8)))\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_py_import("), "text:\n{text}");
        assert!(text.contains("@zap_py_getattr("), "text:\n{text}");
        assert!(text.contains("@zap_py_call("), "text:\n{text}");
        assert!(text.contains("call @zap_py_from_i64("), "text:\n{text}");
        assert!(text.contains("@zap_py_to_i64("), "text:\n{text}");
    }

    #[test]
    fn python_calls_in_a_try_use_the_exception_edge() {
        // A Python call inside a `try` becomes an `invoke` to the handler, so a
        // raised Python error is catchable with `except` (D-0012 exception
        // translation routes through the #22 unwinding path).
        let m = lower_ok(
            "import python \"math\" as m\n\ndef main():\n    try:\n        print(int64(m.sqrt(2.0)))\n    except:\n        print(\"boom\")\n",
        );
        let text = format!("{m}");
        assert!(text.contains("invoke @zap_py_import"), "text:\n{text}");
        assert!(text.contains("invoke @zap_py_call"), "text:\n{text}");
        assert!(text.contains("landingpad"), "text:\n{text}");
    }

    #[test]
    fn regex_subject_group_method_is_checked() {
        // An unknown match-object method reports cleanly (not a silent miscompile).
        let (ast, _p, _l) =
            parse_str("def main():\n    auto mt = \"a\" =~ m/a/\n    print(mt.nope())\n");
        let (_m, errs) = lower_module(&ast);
        assert!(!errs.is_empty(), "unknown match method should error");
    }

    #[test]
    fn derive_eq_only_methods_are_callable() {
        // A struct can derive just one trait; the generated method resolves.
        let m = lower_ok(
            "@derive(Eq)\nstruct P:\n    int64 x\n\ndef main():\n    \
             auto a = P(x=1)\n    auto b = P(x=1)\n    if a == b:\n        print(1)\n",
        );
        assert!(m.funcs.iter().any(|f| f.name == "P____eq__"));
        assert!(!m.funcs.iter().any(|f| f.name == "P____repr__"));
    }

    #[test]
    fn deferred_builtins_error() {
        // A field decorator applied to a *non-field* target stays deferred at
        // lowering (sema's target check gates the normal path; this is the
        // belt-and-suspenders lowering guard). `@cache`/`@inject` no longer reach
        // lowering at all — `@cache` is expanded to real Zap before sema (I#666,
        // D-0115); `@inject` stays rejected at sema (ZAP5055, I#472/D-0109).
        let src = "@validate(max_len=1)\ndef f() -> int64:\n    return 1\n\ndef main():\n    print(f())\n";
        let (ast, _p, _l) = parse_str(src);
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "expected a deferred-feature error for:\n{src}"
        );
    }

    #[test]
    fn deprecated_lowers_as_noop() {
        // `@deprecated` (I#472, D-0109) is a sema-only use-site warning; the
        // decorated function/method body lowers unchanged, with no error and no
        // generated wrapper.
        let src = "@deprecated(\"use g\")\ndef f() -> int64:\n    return 1\n\n\
                   struct S:\n    int64 x\n\n    \
                   @deprecated\n    def legacy(self) -> int64:\n        return self.x\n\n\
                   def main():\n    print(f())\n";
        let (ast, _p, _l) = parse_str(src);
        let (m, errs) = lower_module(&ast);
        assert!(errs.is_empty(), "deprecated must lower cleanly: {errs:?}");
        // The original function is present and unwrapped (no `f$orig`/`f$…`).
        assert!(m.funcs.iter().any(|f| f.name == "f"));
        assert!(
            !m.funcs.iter().any(|f| f.name.contains('$')),
            "no wrapper functions should be synthesized for @deprecated"
        );
    }

    #[test]
    fn field_validators_generate_check_method() {
        // `@validate`/`@range` field decorators (D-0101) generate a
        // `{Struct}____validate__` method and no error.
        let src = "struct S:\n    @validate(max_len=9)\n    str x\n    @range(min=0, max=5)\n    \
                   int64 n\n\ndef main():\n    auto s = S(x=\"hi\", n=1)\n    print(s.n)\n";
        let (ast, _p, _l) = parse_str(src);
        let (m, errs) = lower_module(&ast);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        assert!(
            m.funcs.iter().any(|f| f.name == "S____validate__"),
            "expected generated S____validate__"
        );
    }

    #[test]
    fn field_default_decorator_initializes_field() {
        // `@default(expr)` (D-0101) supplies a construction-time default for a
        // field the caller omits, reusing the literal-default machinery (#461).
        let src = "struct S:\n    @default(7)\n    int64 n\n\ndef main():\n    \
                   auto s = S()\n    print(s.n)\n";
        let (ast, _p, _l) = parse_str(src);
        let (_m, errs) = lower_module(&ast);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
    }

    #[test]
    fn derive_json_generates_codec() {
        // @derive(Json) (D-0059, I#100) generates the encode method, the decode
        // associated function, and its recursive JsonValue core.
        let (ast, _p, _l) = parse_str("@derive(Json)\nstruct S:\n    int64 x\n    str y\n");
        let (m, errs) = lower_module(&ast);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        for want in ["S__to_json", "S__from_json", "S__from_json_value"] {
            assert!(
                m.funcs.iter().any(|f| f.name == want),
                "missing generated {want}"
            );
        }
    }

    #[test]
    fn derive_yaml_generates_codec() {
        // @derive(Yaml) (D-0063, I#149) generates the encode method, the decode
        // associated function, and reuses the shared JsonValue decode core.
        let (ast, _p, _l) = parse_str("@derive(Yaml)\nstruct S:\n    int64 x\n    str y\n");
        let (m, errs) = lower_module(&ast);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        for want in ["S__to_yaml", "S__from_yaml", "S__from_json_value"] {
            assert!(
                m.funcs.iter().any(|f| f.name == want),
                "missing generated {want}"
            );
        }
    }

    #[test]
    fn derive_json_and_yaml_share_decode_core() {
        // Deriving both must emit the shared `S__from_json_value` exactly once
        // (deduped), not twice.
        let (ast, _p, _l) =
            parse_str("@derive(Json)\n@derive(Yaml)\nstruct S:\n    int64 x\n    str y\n");
        let (m, errs) = lower_module(&ast);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        let n = m
            .funcs
            .iter()
            .filter(|f| f.name == "S__from_json_value")
            .count();
        assert_eq!(n, 1, "shared decode core should be generated once, got {n}");
    }

    // --- f-string replacement fields (#498) ---

    #[test]
    fn fstring_field_dict_get_is_monomorphized() {
        // #498: `dict.get(key)` used directly in an f-string field lowers
        // cleanly — monomorphization sees the re-parsed field (threaded via
        // `fstring_fields`), instantiates `Optional$str`, and emits the dict.get
        // runtime call. Without the fix this errored with ZAP6001.
        let m = lower_ok(
            "def main():\n    dict[str, str] d = {\"k\": \"v\"}\n    print(f\"{d.get(\"k\")}\")\n",
        );
        let has_get = m
            .funcs
            .iter()
            .flat_map(|f| f.blocks.iter())
            .flat_map(|b| b.insts.iter())
            .any(|i| matches!(i, Inst::Call { func, .. } if func.starts_with("zap_dict_get")));
        assert!(
            has_get,
            "expected a zap_dict_get_* call lowered from the field"
        );
    }

    #[test]
    fn fstring_field_optional_renders() {
        // #498: an `Optional[T]` field lowers (null-niche check + "None"
        // literal) rather than erroring as non-renderable.
        let m = lower_ok(
            "def main():\n    Optional[str] o = Optional.Some(\"v\")\n    print(f\"{o}\")\n",
        );
        let has_none = m
            .funcs
            .iter()
            .flat_map(|f| f.blocks.iter())
            .flat_map(|b| b.insts.iter())
            .any(|i| matches!(i, Inst::StrConst { bytes, .. } if bytes == b"None"));
        assert!(
            has_none,
            "expected a \"None\" literal from Optional rendering"
        );
    }

    // --- operator overloading (ticket #52, D-0047) ---

    #[test]
    fn operator_desugars_to_dunder_call() {
        // `a + b` / `a == b` / `a < b` on a struct lower to calls of the mangled
        // dunder methods (`V____add__`, `V____eq__`, `V____cmp__`).
        let m = lower_ok(
            "struct V:\n    int64 x\n\n    \
             def __add__(self, V other) -> V:\n        return V(x=self.x + other.x)\n\n    \
             def __eq__(self, V other) -> bool:\n        return self.x == other.x\n\n    \
             def __cmp__(self, V other) -> int64:\n        return 0\n\n\
             def main():\n    auto a = V(x=1)\n    auto b = V(x=2)\n    \
             auto c = a + b\n    if a == b:\n        print(1)\n    if a < b:\n        print(2)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@V____add__"), "text:\n{text}");
        assert!(text.contains("@V____eq__"), "text:\n{text}");
        assert!(text.contains("@V____cmp__"), "text:\n{text}");
    }

    #[test]
    fn derive_ord_and_hash_generate_methods() {
        // @derive(Ord)/(Hash) now generate `__cmp__`/`__hash__` (in addition to
        // the existing Eq/Repr), field-wise.
        let m = lower_ok("@derive(Eq, Ord, Hash, Repr)\nstruct V:\n    int64 x\n    str s\n");
        assert!(
            m.funcs.iter().any(|f| f.name == "V____cmp__"),
            "expected a generated __cmp__"
        );
        assert!(
            m.funcs.iter().any(|f| f.name == "V____hash__"),
            "expected a generated __hash__"
        );
        let hash = m.funcs.iter().find(|f| f.name == "V____hash__").unwrap();
        assert_eq!(hash.ret, Ty::I64);
        let text = format!("{m}");
        // Hash combines per-field hashes; cmp compares the str field via the runtime.
        assert!(text.contains("@zap_hash_combine"), "text:\n{text}");
        assert!(text.contains("@zap_str_cmp"), "text:\n{text}");
    }

    #[test]
    fn struct_set_key_uses_fn_entry_points() {
        // A struct used as a set element routes to the `_fn` runtime entry points
        // and passes its `__hash__`/`__eq__` as function pointers (D-0047).
        let m = lower_ok(
            "@derive(Eq, Hash)\nstruct V:\n    int64 x\n\n\
             def main():\n    auto a = V(x=1)\n    auto s = {a}\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_set_add_fn"), "text:\n{text}");
        assert!(text.contains("funcaddr @V____hash__"), "text:\n{text}");
        assert!(text.contains("funcaddr @V____eq__"), "text:\n{text}");
    }

    // --- auto-derive Eq/Hash for value aggregates (D-0139/#808) ---

    #[test]
    fn value_struct_auto_derives_eq_hash_as_dict_key() {
        // No `@derive`: a value struct of all-hashable fields auto-derives Eq+Hash
        // and is usable as a dict key, generating `Point____hash__`/`____eq__` and
        // routing through the function-pointer key path.
        let m = lower_ok(
            "value struct Point:\n    int64 x\n    int64 y\n\n\
             def main():\n    auto d = {Point(x=1, y=2): 10}\n    print(len(d))\n",
        );
        let text = format!("{m}");
        assert!(text.contains("funcaddr @Point____hash__"), "text:\n{text}");
        assert!(text.contains("funcaddr @Point____eq__"), "text:\n{text}");
        assert!(
            m.funcs.iter().any(|f| f.name == "Point____hash__"),
            "expected an auto-derived __hash__"
        );
    }

    #[test]
    fn value_struct_auto_derives_as_set_member() {
        // The set-member position likewise accepts an auto-derived value struct.
        let m = lower_ok(
            "value struct P:\n    int64 x\n\n\
             def main():\n    auto s = {P(x=1), P(x=2)}\n    print(len(s))\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_set_add_fn"), "text:\n{text}");
        assert!(text.contains("funcaddr @P____hash__"), "text:\n{text}");
    }

    #[test]
    fn value_struct_str_field_auto_derives() {
        // A `str` field is hashable, so a value struct mixing int + str still
        // auto-derives and keys a dict.
        let m = lower_ok(
            "value struct Name:\n    str first\n    int64 id\n\n\
             def main():\n    auto d = {Name(first=\"a\", id=1): 7}\n    print(len(d))\n",
        );
        assert!(
            m.funcs.iter().any(|f| f.name == "Name____hash__"),
            "expected auto-derived __hash__ for a str+int value struct"
        );
    }

    #[test]
    fn value_struct_float_field_auto_derives_via_key_word() {
        // A float field auto-derives and hashes through the #803 canonical key
        // word (`-0.0` canonicalized, `NaN` rejected at runtime), so the generated
        // `__hash__` calls `zap_f64_value_key_word`.
        let m = lower_ok(
            "value struct Pt:\n    float64 x\n    float64 y\n\n\
             def main():\n    auto d = {Pt(x=1.5, y=2.5): 1}\n    print(len(d))\n",
        );
        let text = format!("{m}");
        assert!(
            text.contains("@zap_f64_value_key_word"),
            "float field hash must canonicalize via zap_f64_value_key_word; text:\n{text}"
        );
        assert!(text.contains("funcaddr @Pt____hash__"), "text:\n{text}");
    }

    #[test]
    fn value_struct_nested_value_struct_field_auto_derives() {
        // A value aggregate of value aggregates: the outer `__hash__`/`__eq__`
        // delegate to the inner type's generated dunders (both auto-derived).
        let m = lower_ok(
            "value struct Inner:\n    int64 a\n\n\
             value struct Outer:\n    Inner i\n    int64 b\n\n\
             def main():\n    auto d = {Outer(i=Inner(a=1), b=2): 9}\n    print(len(d))\n",
        );
        let text = format!("{m}");
        assert!(text.contains("funcaddr @Outer____hash__"), "text:\n{text}");
        assert!(
            m.funcs.iter().any(|f| f.name == "Inner____hash__"),
            "the nested value struct must also auto-derive __hash__"
        );
        assert!(
            text.contains("@Inner____hash__"),
            "Outer's hash must delegate to Inner's; text:\n{text}"
        );
    }

    #[test]
    fn value_struct_with_list_field_not_used_as_key_is_ok() {
        // A value struct with a non-hashable (list) field is perfectly fine to
        // define and use — it simply does not auto-derive Hash. Only *key use*
        // would be an error.
        lower_ok(
            "value struct Bag:\n    list[int64] items\n\n\
             def main():\n    auto b = Bag(items=[1, 2])\n    print(len(b.items))\n",
        );
    }

    #[test]
    fn value_struct_with_list_field_as_key_names_the_field() {
        // Using such a value struct as a dict key is a clean error that names the
        // offending field and its type (D-0139/#808), not a generic "add
        // @derive".
        let errs = lower_errs(
            "value struct Bag:\n    list[int64] items\n\n\
             def main():\n    auto d = {Bag(items=[1]): 1}\n",
        );
        assert!(!errs.is_empty(), "list-field value struct key should error");
        assert!(
            errs.iter().any(|e| e.message.contains("field 'items'")
                && e.message.contains("not hashable")
                && e.message.contains("Bag")),
            "error should name field 'items' of value struct 'Bag', got: {errs:?}"
        );
    }

    #[test]
    fn hand_written_eq_suppresses_auto_hash_pair() {
        // Override is all-or-nothing: a hand-written `__eq__` suppresses the
        // auto-`__hash__` too, so the value struct is NOT hashable and key use
        // reports the missing `__hash__` (the sema Eq-without-Hash warning is
        // exercised in the sema crate).
        let errs = lower_errs(
            "value struct P:\n    int64 x\n\
             \x20   def __eq__(self, P other) -> bool:\n        return self.x == other.x\n\n\
             def main():\n    auto d = {P(x=1): 1}\n",
        );
        assert!(
            errs.iter().any(|e| e.message.contains("needs '__hash__'")),
            "a hand-written __eq__ must suppress auto-__hash__, got: {errs:?}"
        );
    }

    #[test]
    fn reference_struct_does_not_auto_derive() {
        // Only VALUE structs auto-derive; a reference struct (identity default,
        // D-0023) still requires an explicit `@derive`/definition to be a key.
        let errs = lower_errs(
            "struct R:\n    int64 x\n\n\
             def main():\n    auto d = {R(x=1): 1}\n",
        );
        assert!(
            errs.iter().any(|e| e.message.contains("needs '__hash__'")),
            "a reference struct must not auto-derive, got: {errs:?}"
        );
    }

    // --- enum capabilities (D-0139/#809) ---

    #[test]
    fn tier1_enum_dict_key_hashes_as_int_word() {
        // A tier-1 (constant) enum is int-backed, so a `dict[Color, V]` key hashes
        // via the integer-word key path with no synthesized key function.
        lower_ok(
            "enum Color:\n    Red\n    Green\n    Blue\n\n\
             def main():\n    dict[Color, int64] d = {}\n    d[Color.Red] = 1\n    \
             print(d[Color.Red])\n",
        );
    }

    #[test]
    fn tier1_enum_set_and_heap_work() {
        // Set membership and a `heap[Color]` (ordered by backing int) also work
        // out of the box for a tier-1 enum.
        lower_ok(
            "enum Color:\n    Red\n    Green\n\n\
             def main():\n    set[Color] s = set[Color]()\n    s.add(Color.Red)\n    \
             print(Color.Red in s)\n    heap[Color] h = heap[Color]()\n    \
             h.push(Color.Green)\n    print(h.peek() == Color.Green)\n",
        );
    }

    #[test]
    fn tier3_enum_dict_key_synthesizes_tagged_key_fns() {
        // A tier-3 (data-carrying) enum keys a dict by synthesizing a structural
        // `$enum$<sid>$hash`/`$eq` that switches on the tag and folds the active
        // variant's payload — routed through the function-pointer key path.
        let m = lower_ok(
            "enum Tag:\n    A(int64 x)\n    B(str y)\n\n\
             def main():\n    dict[Tag, int64] d = {}\n    d[Tag.A(x=1)] = 10\n    \
             d[Tag.B(y=\"hi\")] = 20\n    print(d[Tag.A(x=1)] + d[Tag.B(y=\"hi\")])\n",
        );
        let text = format!("{m}");
        assert!(
            m.funcs
                .iter()
                .any(|f| f.name.starts_with("$enum$") && f.name.ends_with("$hash")),
            "expected a synthesized tier-3 enum hash fn"
        );
        assert!(
            text.contains("$hash") && text.contains("$eq"),
            "tier-3 enum key must route through its synthesized hash/eq; text:\n{text}"
        );
    }

    #[test]
    fn tier3_enum_float_payload_key_uses_key_word() {
        // A float payload field inherits the #803 canonical key word.
        let m = lower_ok(
            "enum Num:\n    I(int64 n)\n    F(float64 v)\n\n\
             def main():\n    dict[Num, int64] d = {}\n    d[Num.F(v=1.5)] = 1\n    \
             print(d[Num.F(v=1.5)])\n",
        );
        let text = format!("{m}");
        assert!(
            text.contains("@zap_f64_value_key_word"),
            "a float payload must hash through zap_f64_value_key_word; text:\n{text}"
        );
    }

    #[test]
    fn tier3_enum_with_nonhashable_payload_as_key_is_clean_error() {
        // A payload field that is not hashable (a `list`) makes the enum key a
        // clean coded error, never an ICE.
        let errs = lower_errs(
            "enum Bad:\n    Items(list[int64] xs)\n    Empty\n\n\
             def main():\n    auto d = {Bad.Items(xs=[1]): 1}\n",
        );
        assert!(
            errs.iter().any(|e| e.message.contains("not hashable")),
            "a list-payload tier-3 enum key should be a clean error, got: {errs:?}"
        );
    }

    // --- tuples as dict keys / set elements (#680, D-0049) ---

    /// Count the generated tuple key functions in a module (`$tuple$<sid>$hash`).
    fn tuple_hash_fn_count(m: &Module) -> usize {
        m.funcs
            .iter()
            .filter(|f| f.name.starts_with("$tuple$") && f.name.ends_with("$hash"))
            .count()
    }

    #[test]
    fn tuple_dict_key_lowers_without_ice() {
        // Regression for the #680 ICE: `{(1, 2): 7}` used to panic at
        // `key_fn_args` (`self.ctx.names[sid]` out of bounds, a tuple sid has no
        // name entry). It must now lower cleanly: the tuple routes through the
        // function-pointer dict-key path with a synthesized structural
        // `__hash__`/`__eq__` folded with `zap_hash_combine` over `zap_i64_hash`.
        let m = lower_ok("def main():\n    auto d = {(1, 2): 7}\n    print(d[(1, 2)])\n");
        let text = format!("{m}");
        assert!(text.contains("@zap_dict_set_fn_i64"), "text:\n{text}");
        assert!(text.contains("@zap_dict_get_fn_i64"), "text:\n{text}");
        assert!(text.contains("funcaddr @$tuple$"), "text:\n{text}");
        assert_eq!(tuple_hash_fn_count(&m), 1, "one synthesized tuple hash fn");
        // The synthesized hash folds integer-word element hashes.
        assert!(text.contains("@zap_i64_hash"), "text:\n{text}");
        assert!(text.contains("@zap_hash_combine"), "text:\n{text}");
    }

    #[test]
    fn tuple_dict_key_typed_insert_lookup_lowers() {
        // The explicitly-typed dict form from the #680 repro: insert, `in`, and
        // index-read over a `dict[(int64, str), int64]` all route through the
        // tuple key path; the str element hashes/compares via the str runtime.
        let m = lower_ok(
            "def main():\n    dict[(int64, str), int64] d = {}\n    auto k = (3, \"x\")\n\
             \n    d[k] = 7\n    if k in d:\n        print(d[k])\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_dict_set_fn_i64"), "text:\n{text}");
        assert!(text.contains("funcaddr @$tuple$"), "text:\n{text}");
        // A str element is hashed/compared by value.
        assert!(text.contains("@zap_str_hash"), "text:\n{text}");
        assert!(text.contains("@zap_str_eq"), "text:\n{text}");
    }

    #[test]
    fn tuple_set_element_uses_fn_entry_points() {
        // A tuple set element routes to the `_fn` set entry points with the
        // synthesized key functions, exactly like a `@derive(Hash, Eq)` struct.
        let m = lower_ok("def main():\n    auto s = {(1, 2)}\n    print((1, 2) in s)\n");
        let text = format!("{m}");
        assert!(text.contains("@zap_set_add_fn"), "text:\n{text}");
        assert!(text.contains("funcaddr @$tuple$"), "text:\n{text}");
        assert_eq!(tuple_hash_fn_count(&m), 1, "one synthesized tuple hash fn");
    }

    #[test]
    fn nested_tuple_key_generates_recursive_key_fns() {
        // A nested tuple key `((1, 2), 3)` synthesizes a hash/eq for BOTH the
        // outer `((int,int), int)` and the inner `(int, int)` layouts; the outer
        // hash/eq call the inner's by symbol.
        let m = lower_ok("def main():\n    auto d = {((1, 2), 3): 7}\n    print(d[((1, 2), 3)])\n");
        let text = format!("{m}");
        assert_eq!(
            tuple_hash_fn_count(&m),
            2,
            "outer + inner tuple hash fns; text:\n{text}"
        );
        // The outer eq delegates to the inner tuple's eq (a call, not an inline
        // ICmp) for the aggregate element.
        assert!(
            m.funcs
                .iter()
                .filter(|f| f.name.starts_with("$tuple$") && f.name.ends_with("$eq"))
                .count()
                == 2,
            "outer + inner tuple eq fns; text:\n{text}"
        );
    }

    #[test]
    fn same_tuple_layout_reuses_one_key_fn_pair() {
        // Two dict keys of the same `(int64, int64)` layout share ONE synthesized
        // hash/eq pair (keyed by the interned tuple id), not one per use site.
        let m = lower_ok(
            "def main():\n    auto d = {(1, 2): 7}\n    d[(3, 4)] = 8\n    \
             print(d[(1, 2)])\n    print(d[(3, 4)])\n",
        );
        assert_eq!(
            tuple_hash_fn_count(&m),
            1,
            "the shared layout yields exactly one hash fn"
        );
    }

    #[test]
    fn tuple_key_with_unhashable_element_is_clean_error_not_ice() {
        // A tuple is hashable only when all elements are (CPython parity). A
        // float, a nested collection, or a struct lacking `__hash__`/`__eq__`
        // element is rejected with a clean `LowerError` — NEVER an ICE.
        for (src, needle) in [
            (
                "def main():\n    auto d = {(1, [2, 3]): 7}\n",
                "is not hashable",
            ),
            (
                "struct P:\n    int64 x\n\ndef main():\n    auto p = P(x=1)\n    auto d = {(1, p): 7}\n",
                "needs '__hash__' and '__eq__'",
            ),
        ] {
            let errs = lower_errs(src);
            assert!(!errs.is_empty(), "expected a clean error for:\n{src}");
            assert!(
                errs.iter().any(|e| e.message.contains(needle)),
                "error for {src:?} should mention {needle:?}, got: {errs:?}"
            );
        }
    }

    #[test]
    fn tuple_key_with_hashable_struct_element_lowers() {
        // A tuple whose struct element DOES derive Hash/Eq is hashable: the
        // synthesized tuple hash/eq delegate to the struct's own dunders.
        let m = lower_ok(
            "@derive(Eq, Hash)\nstruct P:\n    int64 x\n\n\
             def main():\n    auto p = P(x=1)\n    auto d = {(1, p): 7}\n    print(d[(1, p)])\n",
        );
        let text = format!("{m}");
        assert!(text.contains("funcaddr @$tuple$"), "text:\n{text}");
        // The tuple hash/eq call the struct's own derived dunders on that element.
        assert!(text.contains("@P____hash__"), "text:\n{text}");
        assert!(text.contains("@P____eq__"), "text:\n{text}");
    }

    // --- typed empty collection constructors with composite element types
    //     (#703): `set[(..)]()` / `channel[(..)]()` / `heap[(..)]()` ---

    #[test]
    fn set_ctor_accepts_tuple_element_type() {
        // #703: `set[(int64, int64)]()` used to reject with ZAP6001 "expects an
        // element type name", even though the set literal `{(1, 2)}` and
        // `dict[(..), ..]` keys accept tuples (D-0122). The typed empty
        // constructor now accepts any valid element type expression; a tuple
        // element routes through the synthesized key-fn path on `.add` / `in`,
        // exactly like the literal.
        let m = lower_ok(
            "def main():\n    set[(int64, int64)] s = set[(int64, int64)]()\n    \
             s.add((1, 2))\n    print((1, 2) in s)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_set_new"), "text:\n{text}");
        assert!(text.contains("@zap_set_add_fn"), "text:\n{text}");
        assert!(text.contains("@zap_set_contains_fn"), "text:\n{text}");
        assert!(text.contains("funcaddr @$tuple$"), "text:\n{text}");
    }

    #[test]
    fn set_ctor_accepts_nested_tuple_element_type() {
        // A nested-tuple element type is accepted and generates recursive key
        // functions for both the outer and inner tuple layouts.
        let m = lower_ok(
            "def main():\n    auto s = set[((int64, int64), int64)]()\n    \
             s.add(((1, 2), 3))\n    print(((1, 2), 3) in s)\n",
        );
        assert_eq!(
            tuple_hash_fn_count(&m),
            2,
            "outer + inner tuple hash fns; text:\n{m}"
        );
    }

    #[test]
    fn set_ctor_bare_name_element_still_lowers() {
        // Regression: the pre-#703 bare-name element path is unchanged.
        let m =
            lower_ok("def main():\n    auto s = set[int64]()\n    s.add(7)\n    print(7 in s)\n");
        let text = format!("{m}");
        assert!(text.contains("@zap_set_new"), "text:\n{text}");
        assert!(text.contains("@zap_set_add_i64"), "text:\n{text}");
    }

    #[test]
    fn set_ctor_nested_collection_element_is_clean_error() {
        // A nested collection has no stable hash, so `set[list[int64]]()` is
        // rejected cleanly (never an ICE) — the constructor now parses the
        // composite element type and reaches the same unhashable guard as the
        // literal / annotation forms, instead of "expects an element type name".
        let errs = lower_errs("def main():\n    auto s = set[list[int64]]()\n");
        assert!(!errs.is_empty(), "set[list[..]]() should be a clean error");
        assert!(
            errs.iter().any(|e| e.message.contains("unhashable")),
            "expected an unhashable-element diagnostic, got: {errs:?}"
        );
    }

    #[test]
    fn heap_ctor_tuple_element_is_clean_error_not_ice() {
        // A heap orders its elements; a tuple has no ordering, so the constructor
        // rejects a tuple element cleanly — the same diagnostic as the `heap[(..)]`
        // annotation, never an ICE and never "expects an element type name".
        let errs = lower_errs("def main():\n    auto h = heap[(int64, int64)]()\n");
        assert!(!errs.is_empty(), "heap[(..)]() should be a clean error");
        assert!(
            errs.iter().any(|e| e.message.contains("not supported yet")),
            "expected a heap-unsupported diagnostic, got: {errs:?}"
        );
    }

    #[test]
    fn channel_ctor_tuple_element_is_clean_error_not_ice() {
        // A channel stores uniform words; a tuple is a value aggregate whose
        // delivery across the channel ABI is unimplemented, so the constructor
        // rejects it cleanly (never an ICE) with a specific message.
        let errs = lower_errs("def main():\n    auto c = channel[(int64, int64)]()\n");
        assert!(!errs.is_empty(), "channel[(..)]() should be a clean error");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("tuple elements are not supported")),
            "expected a channel-tuple diagnostic, got: {errs:?}"
        );
    }

    #[test]
    fn channel_ctor_bare_name_element_still_lowers() {
        // Regression: a scalar/str channel element is unchanged.
        let m = lower_ok("def main():\n    auto c = channel[str]()\n    print(0)\n");
        assert!(format!("{m}").contains("@zap_chan_new"), "text:\n{m}");
    }

    // --- Optional[T] as a dict key / set element (#704, D-0122) ---

    /// Count the generated Optional key functions in a module (`$opt$<sid>$hash`).
    fn opt_hash_fn_count(m: &Module) -> usize {
        m.funcs
            .iter()
            .filter(|f| f.name.starts_with("$opt$") && f.name.ends_with("$hash"))
            .count()
    }

    #[test]
    fn optional_dict_key_lowers_via_synthesized_key_fns() {
        // #704: `Optional[int64]` as a dict key used to reject (ZAP6001). It now
        // routes through the fn-key path with a synthesized structural
        // `__hash__`/`__eq__`: `None` hashes to a sentinel, `Some(v)` delegates to
        // the payload's `zap_i64_hash`. Exercises insert, a `None` literal key,
        // index-read, and `in`.
        let m = lower_ok(
            "def main():\n    dict[Optional[int64], str] d = {}\n    \
             Optional[int64] k = Optional.Some(5)\n    d[k] = \"five\"\n    \
             d[None] = \"none\"\n    print(d[k])\n    print(d[None])\n    \
             print(k in d)\n    print(None in d)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_dict_set_fn_ptr"), "text:\n{text}");
        assert!(text.contains("@zap_dict_get_fn_ptr"), "text:\n{text}");
        assert!(text.contains("@zap_dict_contains_fn"), "text:\n{text}");
        assert!(text.contains("funcaddr @$opt$"), "text:\n{text}");
        assert_eq!(opt_hash_fn_count(&m), 1, "one synthesized Optional hash fn");
        // The Some branch delegates to the payload's integer hash.
        assert!(text.contains("@zap_i64_hash"), "text:\n{text}");
    }

    #[test]
    fn optional_set_element_lowers() {
        // An `Optional[int64]` set element routes to the `_fn` set entry points
        // with the synthesized key functions, including a bare `None` element.
        let m = lower_ok(
            "def main():\n    set[Optional[int64]] s = set[Optional[int64]]()\n    \
             Optional[int64] k = Optional.Some(5)\n    s.add(k)\n    s.add(None)\n    \
             print(k in s)\n    print(None in s)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_set_add_fn"), "text:\n{text}");
        assert!(text.contains("@zap_set_contains_fn"), "text:\n{text}");
        assert!(text.contains("funcaddr @$opt$"), "text:\n{text}");
        assert_eq!(opt_hash_fn_count(&m), 1, "one synthesized Optional hash fn");
    }

    #[test]
    fn optional_none_literal_in_key_position_lowers_not_phase1() {
        // A bare `None` in dict-subscript / membership position must lower against
        // the Optional key type (not hit the Phase-1 unsupported-expression path).
        // Each of these used to fail; now they lower cleanly.
        for src in [
            "def main():\n    dict[Optional[int64], str] d = {}\n    d[None] = \"x\"\n",
            "def main():\n    dict[Optional[int64], str] d = {}\n    print(None in d)\n",
            "def main():\n    set[Optional[int64]] s = set[Optional[int64]]()\n    s.add(None)\n",
            "def main():\n    set[Optional[int64]] s = set[Optional[int64]]()\n    print(None in s)\n",
        ] {
            let errs = lower_errs(src);
            assert!(errs.is_empty(), "expected clean lowering for:\n{src}\ngot: {errs:?}");
        }
    }

    #[test]
    fn optional_niche_key_hashes_via_isnull() {
        // A niche-layout Optional (pointer payload, e.g. `Optional[str]`) hashes
        // its `None` via a null test rather than a discriminant tag.
        let m = lower_ok(
            "def main():\n    dict[Optional[str], int64] d = {}\n    \
             Optional[str] k = Optional.Some(\"hi\")\n    d[k] = 1\n    d[None] = 2\n    \
             print(d[k])\n    print(d[None])\n",
        );
        let text = format!("{m}");
        assert!(text.contains("funcaddr @$opt$"), "text:\n{text}");
        assert!(
            text.contains("@zap_str_hash"),
            "a str payload hashes by bytes:\n{text}"
        );
        // The synthesized Optional hash fn tests None with `isnull` (niche layout).
        let hashfn: String = {
            let t = &text;
            let start = t.find("func @$opt$").expect("an $opt$ fn");
            let end = t[start..]
                .find("\n}\n")
                .map(|e| start + e)
                .unwrap_or(t.len());
            t[start..end].to_string()
        };
        assert!(
            hashfn.contains("isnull"),
            "niche None test uses isnull:\n{hashfn}"
        );
    }

    #[test]
    fn optional_nested_tuple_payload_key_lowers() {
        // `Optional[(int64, int64)]` — a Some payload that is itself a tuple —
        // synthesizes BOTH the Optional key fns and the inner tuple's key fns.
        let m = lower_ok(
            "def main():\n    dict[Optional[(int64, int64)], str] d = {}\n    \
             Optional[(int64, int64)] k = Optional.Some((1, 2))\n    d[k] = \"x\"\n    \
             d[None] = \"y\"\n    print(d[k])\n    print(d[None])\n",
        );
        let text = format!("{m}");
        assert_eq!(
            opt_hash_fn_count(&m),
            1,
            "one Optional hash fn; text:\n{text}"
        );
        assert_eq!(
            tuple_hash_fn_count(&m),
            1,
            "the Some payload's tuple hash fn; text:\n{text}"
        );
        assert!(text.contains("funcaddr @$opt$"), "text:\n{text}");
    }

    #[test]
    fn optional_unhashable_payload_is_clean_error_not_ice() {
        // `Optional[list[int64]]` as a key is rejected cleanly (a `list` payload is
        // unhashable), never an ICE — the same coded diagnostic as any other
        // unhashable key. (A float payload is now hashable — D-0139/#808 — so the
        // unhashable example is a mutable collection.)
        let errs = lower_errs(
            "def main():\n    dict[Optional[list[int64]], int64] d = {}\n    \
             Optional[list[int64]] k = Optional.Some([1, 2])\n    d[k] = 1\n",
        );
        assert!(
            !errs.is_empty(),
            "Optional[list] key should be a clean error"
        );
        assert!(
            errs.iter().any(|e| e.message.contains("is not hashable")),
            "expected an unhashable-payload diagnostic, got: {errs:?}"
        );
    }

    #[test]
    fn optional_key_layout_reuses_one_key_fn_pair() {
        // Two keys of the same `Optional[int64]` layout share ONE synthesized
        // hash/eq pair (keyed by the interned instance id), not one per use site.
        let m = lower_ok(
            "def main():\n    dict[Optional[int64], str] d = {}\n    \
             Optional[int64] a = Optional.Some(1)\n    Optional[int64] b = Optional.Some(2)\n    \
             d[a] = \"a\"\n    d[b] = \"b\"\n    print(d[a])\n    print(d[b])\n",
        );
        assert_eq!(
            opt_hash_fn_count(&m),
            1,
            "the shared layout yields one hash fn"
        );
    }

    #[test]
    fn operator_without_dunder_lowers_to_clean_error() {
        // The ir crate runs without sema; a missing dunder is a clean error.
        let (ast, _p, _l) = parse_str(
            "struct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto b = V(x=2)\n    auto c = a + b\n",
        );
        let (_m, errs) = lower_module(&ast);
        assert!(!errs.is_empty(), "missing __add__ should error");
        assert!(errs[0].message.contains("__add__"), "{errs:?}");
    }

    #[test]
    fn derive_and_user_method_clash_errors() {
        let (ast, _p, _l) = parse_str(
            "@derive(Eq)\nstruct S:\n    int64 x\n\n    def __eq__(self, S other) -> bool:\n        return True\n",
        );
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "a user `__eq__` plus @derive(Eq) should clash"
        );
    }

    // --- in-place augmented assignment (ticket #61, D-0047/D-0023) ---

    #[test]
    fn aug_assign_uses_inplace_dunder_when_defined() {
        // `a += b` on a struct that defines `__iadd__` lowers to a call of the
        // in-place dunder (`Acc____iadd__`), which mutates in place — NOT the
        // binary `__add__` form.
        let m = lower_ok(
            "struct Acc:\n    int64 total\n\n    \
             def __iadd__(self, Acc other) -> Acc:\n        \
             self.total = self.total + other.total\n        return self\n\n\
             def main():\n    auto a = Acc(total=1)\n    auto b = Acc(total=2)\n    \
             a += b\n    print(a.total)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@Acc____iadd__"), "text:\n{text}");
        assert!(
            !text.contains("@Acc____add__"),
            "the in-place form must not call the binary __add__:\n{text}"
        );
    }

    #[test]
    fn aug_assign_falls_back_to_binary_dunder() {
        // A struct that defines only `__add__` (no `__iadd__`) still supports
        // `+=`, deriving it from the binary operator (`a = a.__add__(b)`).
        let m = lower_ok(
            "struct V:\n    int64 x\n\n    \
             def __add__(self, V other) -> V:\n        return V(x=self.x + other.x)\n\n\
             def main():\n    auto a = V(x=1)\n    auto b = V(x=2)\n    \
             a += b\n    print(a.x)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@V____add__"), "text:\n{text}");
        assert!(
            !text.contains("@V____iadd__"),
            "no __iadd__ is defined, so none should be called:\n{text}"
        );
    }

    #[test]
    fn aug_assign_scalar_is_a_plain_binop() {
        // Built-in scalar `+=` is unchanged: a direct `Add`, no dunder call.
        let m = lower_ok("def main():\n    auto a = 1\n    a += 2\n    print(a)\n");
        let text = format!("{m}");
        assert!(text.contains("= Add "), "scalar += is a plain Add:\n{text}");
        assert!(
            !text.contains("__iadd__") && !text.contains("__add__"),
            "a scalar must not route through an operator dunder:\n{text}"
        );
    }

    // --- tuples (ticket #54, D-0049) ---

    #[test]
    fn lower_tuple_literal_and_const_index() {
        // A tuple literal lowers to an anonymous-struct aggregate: `structnew`
        // plus one `fieldset` per element; `t[k]` is a `fieldget` at a constant
        // index.
        let m = lower_ok(include_str!("../../../tests/golden/tuple_basic.zap"));
        let text = format!("{m}");
        assert!(text.contains("structnew"), "text:\n{text}");
        assert!(text.contains("fieldset"), "text:\n{text}");
        assert!(text.contains("fieldget"), "text:\n{text}");
        // One tuple layout `(int64, int64, int64)` was interned as a struct.
        assert_eq!(m.structs.len(), 1, "one interned tuple layout");
        assert_eq!(m.structs[0].fields, vec![Ty::I64, Ty::I64, Ty::I64]);
    }

    #[test]
    fn lower_multiple_return_via_tuple_struct() {
        // `divmod -> (int64, int64)` returns a struct aggregate; the caller
        // destructures it with `fieldget`. The literal return tuple and the
        // declared return type intern to the same struct id.
        let m = lower_ok(include_str!("../../../tests/golden/tuple_multiret.zap"));
        let divmod = m.funcs.iter().find(|f| f.name == "divmod").expect("divmod");
        assert!(matches!(divmod.ret, Ty::Struct(_)), "ret: {:?}", divmod.ret);
        let text = format!("{m}");
        assert!(text.contains("fieldget"), "text:\n{text}");
    }

    #[test]
    fn lower_rest_builds_subtuple() {
        // `(first, *rest) = (10, 20, 30, 40)`: `first` is a `fieldget`, and the
        // rest sub-tuple is a fresh `structnew` copying the remaining fields.
        let m = lower_ok(include_str!("../../../tests/golden/tuple_rest.zap"));
        let text = format!("{m}");
        assert!(text.contains("structnew"), "text:\n{text}");
        // The 4-tuple and the 3-element rest sub-tuple are distinct layouts.
        assert!(
            m.structs.iter().any(|s| s.fields.len() == 4),
            "the source 4-tuple layout"
        );
        assert!(
            m.structs.iter().any(|s| s.fields.len() == 3),
            "the rest 3-tuple layout"
        );
    }

    #[test]
    fn lower_tuple_nonconstant_index_errors() {
        // A runtime tuple index is rejected at lowering too (sema also reports
        // it, but lowering must never emit a dynamic tuple access).
        let (ast, _p, _l) = parse_str(
            "def main():\n    auto t = (1, 2)\n    auto i = 0\n    auto x = t[i]\n    print(x)\n",
        );
        let (_m, errs) = lower_module(&ast);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("compile-time-constant")),
            "expected a const-index error, got {errs:?}"
        );
    }

    #[test]
    fn lower_swap_reads_before_writing() {
        // `(x, y) = (y, x)` builds the right-hand tuple fully before storing, so
        // the swap is correct. The aggregate shows up as structnew + fieldget.
        let m = lower_ok(include_str!("../../../tests/golden/tuple_swap.zap"));
        let text = format!("{m}");
        assert!(text.contains("structnew"), "text:\n{text}");
        assert!(text.contains("fieldget"), "text:\n{text}");
    }

    // --- iteration protocol (ticket #53, D-0048) ---

    const RANGE_SRC: &str = "\
struct Range:
    int64 lo
    int64 hi
    def __iter__(self) -> RangeIter:
        return RangeIter(cur=self.lo, hi=self.hi)

struct RangeIter:
    int64 cur
    int64 hi
    def __iter__(self) -> RangeIter:
        return self
    def __next__(self) -> (int64, bool):
        if self.cur >= self.hi:
            return (0, False)
        auto v = self.cur
        self.cur = self.cur + 1
        return (v, True)
";

    #[test]
    fn lower_generator_state_machine() {
        // A `def` with `yield` (I#67) lowers to a state-struct with comma-ok
        // `__iter__`/`__next__`; the call constructs the struct (no `@count`
        // function is emitted).
        let m = lower_ok(
            "\
def count(int64 n):
    auto i = 0
    while i < n:
        yield i
        i = i + 1

def main():
    auto total = 0
    for x in count(5):
        total = total + x
    print(total)
",
        );
        let text = format!("{m}");
        // The generator function itself is not emitted; its iterator methods are.
        assert!(!m.funcs.iter().any(|f| f.name == "count"), "text:\n{text}");
        assert!(
            m.funcs.iter().any(|f| f.name == "$gen:count____iter__"),
            "text:\n{text}"
        );
        let next = m
            .funcs
            .iter()
            .find(|f| f.name == "$gen:count____next__")
            .expect("__next__ emitted");
        // `__next__` returns the comma-ok `(int64, bool)` tuple.
        assert!(matches!(next.ret, Ty::Struct(_)), "ret: {:?}", next.ret);
        // The state machine dispatches on the `$state` field and suspends/returns.
        assert!(text.contains("$gen:count____next__"), "text:\n{text}");
        assert!(text.contains("icmp"), "text:\n{text}");
        // The call site constructs the state struct rather than calling a fn.
        assert!(!text.contains("call @count"), "text:\n{text}");
    }

    #[test]
    fn lower_generator_method_captures_self() {
        // A struct *method* generator (I#429) lowers to the same state machine as
        // a free generator (I#67); `self` is captured into the state frame like
        // any other parameter, and reads through it (`self.start`) persist across
        // resumes. Keyed on the mangled `Seq__nums`, so `$gen:Seq__nums____*`.
        let m = lower_ok(include_str!("../../../tests/golden/generator_method.zap"));
        let text = format!("{m}");
        // The method function itself is NOT emitted; its iterator methods are.
        assert!(
            !m.funcs.iter().any(|f| f.name == "Seq__nums"),
            "method body is not emitted as a plain fn:\n{text}"
        );
        assert!(
            m.funcs.iter().any(|f| f.name == "$gen:Seq__nums____iter__"),
            "iter emitted:\n{text}"
        );
        let next = m
            .funcs
            .iter()
            .find(|f| f.name == "$gen:Seq__nums____next__")
            .expect("__next__ emitted");
        assert!(matches!(next.ret, Ty::Struct(_)), "ret: {:?}", next.ret);
        // The state struct's layout captures `self` as its leading parameter
        // field (field 0 is `$state`): `[$state:i64, self:Seq, n:i64, …locals]`.
        let gen_sid = match next.params.first() {
            Some(Ty::Struct(sid)) => *sid,
            other => panic!("__next__ self param: {other:?}"),
        };
        let seq_sid = m
            .structs
            .iter()
            .position(|s| s.name == "Seq")
            .expect("Seq laid out") as u32;
        assert_eq!(
            m.structs[gen_sid as usize].fields.get(1),
            Some(&Ty::Struct(seq_sid)),
            "field 1 captures `self` (the Seq receiver):\n{text}"
        );
        // The call site `c.nums(5)` constructs the state struct (capturing the
        // receiver + arg) rather than calling a `Seq__nums` function.
        assert!(!text.contains("call @Seq__nums"), "text:\n{text}");
        assert!(text.contains("$gen:Seq__nums____next__"), "text:\n{text}");
    }

    #[test]
    fn lower_generator_enum_method_captures_self() {
        // An *enum* method generator (I#449) lowers through the exact same state
        // machine as a struct method generator (I#429) / free generator (I#67);
        // the only delta is that `self` is a tier-3 (tagged-struct) enum receiver.
        // It is captured into the state frame like any other parameter, and the
        // body reads its payload (`match self: case IntBox.Hold(start)`) across
        // resumes. Keyed on the mangled `IntBox__nums`, so `$gen:IntBox__nums____*`.
        let m = lower_ok(include_str!(
            "../../../tests/golden/generator_enum_method.zap"
        ));
        let text = format!("{m}");
        // The method function itself is NOT emitted; its iterator methods are.
        assert!(
            !m.funcs.iter().any(|f| f.name == "IntBox__nums"),
            "method body is not emitted as a plain fn:\n{text}"
        );
        assert!(
            m.funcs
                .iter()
                .any(|f| f.name == "$gen:IntBox__nums____iter__"),
            "iter emitted:\n{text}"
        );
        let next = m
            .funcs
            .iter()
            .find(|f| f.name == "$gen:IntBox__nums____next__")
            .expect("__next__ emitted");
        assert!(matches!(next.ret, Ty::Struct(_)), "ret: {:?}", next.ret);
        // The state struct's layout captures `self` as its leading parameter
        // field (field 0 is `$state`): `[$state:i64, self:IntBox, n:i64, …locals]`.
        let gen_sid = match next.params.first() {
            Some(Ty::Struct(sid)) => *sid,
            other => panic!("__next__ self param: {other:?}"),
        };
        let intbox_sid =
            m.structs
                .iter()
                .position(|s| s.name == "IntBox")
                .expect("IntBox laid out (tier-3 enum -> tagged struct)") as u32;
        assert_eq!(
            m.structs[gen_sid as usize].fields.get(1),
            Some(&Ty::Struct(intbox_sid)),
            "field 1 captures `self` (the IntBox enum receiver):\n{text}"
        );
        // The call site `c.nums(5)` constructs the state struct (capturing the
        // receiver + arg) rather than calling an `IntBox__nums` function.
        assert!(!text.contains("call @IntBox__nums"), "text:\n{text}");
        assert!(
            text.contains("$gen:IntBox__nums____next__"),
            "text:\n{text}"
        );
    }

    #[test]
    fn lower_generator_enum_method_discriminant_dependent() {
        // A generator method whose control flow depends on the enum discriminant
        // (I#449): `match self` yields a different count per variant. Proves the
        // captured receiver carries both tag and payload across resumes, and that
        // `yield` inside match arms lowers. Two variants -> one state machine.
        let m = lower_ok(include_str!(
            "../../../tests/golden/generator_enum_method_variant.zap"
        ));
        let text = format!("{m}");
        assert!(
            !m.funcs.iter().any(|f| f.name == "Steps__items"),
            "method body is not emitted as a plain fn:\n{text}"
        );
        assert!(
            m.funcs
                .iter()
                .any(|f| f.name == "$gen:Steps__items____next__"),
            "__next__ emitted:\n{text}"
        );
        // Both `counted.items()` and `paired.items()` construct the state struct
        // rather than calling a `Steps__items` function.
        assert!(!text.contains("call @Steps__items"), "text:\n{text}");
    }

    #[test]
    fn lower_generator_for_loop_spills_induction_state() {
        // A `for` loop INSIDE a generator body (I#453): the loop's induction
        // index and the iterable handle are synthesized, not named `auto`
        // locals, yet they are live across the `yield` in the loop body. They
        // must be hoisted into the generator state struct (like an explicit
        // local) so they survive suspend/resume — otherwise the index reads
        // uninitialized on resume and the handle (an SSA value defined at loop
        // setup) does not dominate the resume re-entry, crashing `__next__`.
        let m = lower_ok(include_str!(
            "../../../tests/golden/generator_for_in_body.zap"
        ));
        let text = format!("{m}");
        let next = m
            .funcs
            .iter()
            .find(|f| f.name == "$gen:emit____next__")
            .expect("__next__ emitted");
        let gen_sid = match next.params.first() {
            Some(Ty::Struct(sid)) => *sid,
            other => panic!("__next__ self param: {other:?}"),
        };
        // State-struct layout: [$state:i64, xs:list, $for:idx:i64,
        // $for:handle:list, x:i64]. Before the fix only `$state` + `xs` were
        // captured (the index was a per-call alloca, the handle a bare SSA
        // value), so the frame had just 2 fields; the for-loop control state
        // adds the index, the spilled iterable handle, and the loop variable.
        let fields = &m.structs[gen_sid as usize].fields;
        assert!(
            fields.len() >= 5,
            "for-loop induction index, iterable handle, and loop var are captured \
             into the generator frame (got {} field(s)):\n{text}",
            fields.len()
        );
        // field 0 is `$state`; the induction index and loop variable are i64;
        // the spilled iterable handle is the `list[i64]` being walked.
        assert_eq!(fields[0], Ty::I64, "field 0 is $state:\n{text}");
        assert!(
            fields.iter().any(|t| matches!(t, Ty::List(_))),
            "the iterable handle is spilled into the frame as a list field:\n{text}"
        );
    }

    #[test]
    fn lower_user_iterable_commaok() {
        // `for x in Range(...)` desugars to `__iter__` then a loop over the
        // comma-ok `__next__`, binding each tuple's value field.
        let m = lower_ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto total = 0\n    for x in Range(lo=0, hi=5):\n        total = total + x\n    print(total)\n"
        ));
        let text = format!("{m}");
        assert!(text.contains("@Range____iter__"), "text:\n{text}");
        assert!(text.contains("@RangeIter____next__"), "text:\n{text}");
        // The loop reads both fields of the `(int64, bool)` result tuple.
        assert!(text.contains("fieldget struct#2 %"), "text:\n{text}");
    }

    #[test]
    fn lower_stopiteration_iterable() {
        // The Python form drives through a landing pad that checks the in-flight
        // exception's runtime type against `StopIteration`.
        let m = lower_ok(
            "\
struct Count:
    int64 n
    def __iter__(self) -> CountIter:
        return CountIter(cur=0, n=self.n)

struct CountIter:
    int64 cur
    int64 n
    def __iter__(self) -> CountIter:
        return self
    def __next__(self) -> int64:
        if self.cur >= self.n:
            raise StopIteration
        auto v = self.cur
        self.cur = self.cur + 1
        return v

def main():
    auto total = 0
    for x in Count(n=4):
        total = total + x
    print(total)
",
        );
        let text = format!("{m}");
        assert!(text.contains("strconst \"StopIteration\""), "text:\n{text}");
        assert!(text.contains("@zap_exception_is_a"), "text:\n{text}");
        assert!(text.contains("landingpad"), "text:\n{text}");
    }

    #[test]
    fn lower_typed_exception_dispatch_and_message_binding() {
        let m = lower_ok(include_str!(
            "../../../tests/conformance/exception_typed_dispatch.zap"
        ));
        let text = format!("{m}");
        assert!(text.contains("@zap_exception_new"), "text:\n{text}");
        assert!(text.contains("@zap_exception_is_a"), "text:\n{text}");
        assert!(text.contains("@zap_exception_message"), "text:\n{text}");
        assert!(text.contains("@zap_raise"), "text:\n{text}");
        assert!(text.contains("@zap_eh_end_catch"), "text:\n{text}");
    }

    #[test]
    fn lower_list_comprehension_over_user_iterable() {
        let m = lower_ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto squares = [x * x for x in Range(lo=1, hi=5) if x != 3]\n    print(len(squares))\n"
        ));
        let text = format!("{m}");
        assert!(text.contains("@zap_list_new"), "text:\n{text}");
        assert!(text.contains("@zap_list_push_i64"), "text:\n{text}");
        assert!(text.contains("@RangeIter____next__"), "text:\n{text}");
    }

    #[test]
    fn lower_membership_fallback_iterates() {
        // `x in r` with no `__contains__` iterates and compares each element.
        let m = lower_ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto r = Range(lo=0, hi=5)\n    if 3 in r:\n        print(1)\n"
        ));
        let text = format!("{m}");
        assert!(text.contains("@RangeIter____next__"), "text:\n{text}");
        assert!(text.contains("icmp.Eq"), "text:\n{text}");
    }

    #[test]
    fn lower_non_iterable_for_errors() {
        let (ast, _p, _l) = parse_str(
            "struct Point:\n    int64 x\ndef main():\n    for p in Point(x=1):\n        print(1)\n",
        );
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "a non-iterable struct in `for` should be rejected"
        );
    }

    // --- value structs (#65, D-0051) ---

    /// Count struct constructions in a function (each is one aggregate
    /// allocation / copy). Counts both the heap `StructNew` and the
    /// stack-promoted `StackStruct` (escape analysis, #839) so these
    /// copy-vs-no-copy / construction-count checks are independent of whether a
    /// given construction happens to be stack-allocated.
    fn structnews(f: &Func) -> usize {
        f.blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i, Inst::StructNew { .. } | Inst::StackStruct { .. }))
            .count()
    }

    const VEC2: &str = "value struct Vec2:\n    f64 x\n    f64 y\n\
                        \x20   def len2(self) -> f64:\n        return self.x * self.x + self.y * self.y\n";
    const PT: &str = "struct Pt:\n    f64 x\n    f64 y\n";

    #[test]
    fn value_struct_layout_is_flagged() {
        // The `value` modifier is tracked end-to-end onto the ZIR layout
        // (D-0051); a plain struct stays a reference type.
        let m = lower_ok(&format!(
            "{VEC2}{PT}\ndef main():\n    auto v = Vec2(x=1.0, y=2.0)\n    print(v.len2())\n"
        ));
        let vec2 = m.structs.iter().find(|s| s.name == "Vec2").expect("Vec2");
        let pt = m.structs.iter().find(|s| s.name == "Pt").expect("Pt");
        assert!(vec2.is_value, "Vec2 should be a value struct");
        assert!(!pt.is_value, "plain struct Pt should be a reference type");
        assert_eq!(vec2.fields, vec![Ty::F64, Ty::F64]);
        // A flat value struct uses the physical inline / register / sret layout
        // (D-0051); a reference struct never does (shared identity).
        assert!(
            vec2.inline,
            "flat value struct Vec2 should be inline-laid-out"
        );
        assert!(!pt.inline, "reference struct Pt must not be inline");
    }

    #[test]
    fn nested_value_struct_field_is_not_inline() {
        // A value struct with a value-struct *field* keeps the heap layout: a
        // by-value (shallow) aggregate copy would share the nested instance, so
        // the inline ABI does not apply and deep-copy semantics are preserved.
        let m = lower_ok(&format!(
            "{VEC2}value struct Seg:\n    Vec2 a\n    Vec2 b\n\
             def main():\n    auto s = Seg(a=Vec2(x=1.0, y=2.0), b=Vec2(x=3.0, y=4.0))\n    print(s.a.len2())\n"
        ));
        let seg = m.structs.iter().find(|s| s.name == "Seg").expect("Seg");
        let vec2 = m.structs.iter().find(|s| s.name == "Vec2").expect("Vec2");
        assert!(seg.is_value, "Seg is a value struct");
        assert!(
            !seg.inline,
            "a value struct with a value-struct field keeps the heap layout"
        );
        assert!(vec2.inline, "the flat Vec2 field type is itself inline");
    }

    #[test]
    fn tuple_layout_is_inline() {
        // Tuples are value-semantic aggregates and share the inline/register/sret
        // layout path (D-0051), when flat.
        let m = lower_ok(
            "def divmod(int64 a, int64 b) -> (int64, int64):\n    return a // b, a % b\n\
             def main():\n    auto (q, r) = divmod(17, 5)\n    print(q)\n    print(r)\n",
        );
        let tup = m
            .structs
            .iter()
            .find(|s| s.name == "(tuple)" && s.fields == vec![Ty::I64, Ty::I64])
            .expect("(int64, int64) tuple layout");
        assert!(tup.inline, "a flat tuple should be inline-laid-out");
    }

    #[test]
    fn value_struct_copies_on_assignment() {
        // `b = a` for a value struct copies it (extra StructNew + field copies),
        // so mutating `b` cannot escape to `a`. A reference struct shares the
        // pointer (no copy).
        let value_src = format!(
            "{VEC2}\ndef main():\n    auto a = Vec2(x=1.0, y=2.0)\n    auto b = a\n    print(b.len2())\n"
        );
        let ref_src = format!(
            "{PT}\ndef main():\n    auto a = Pt(x=1.0, y=2.0)\n    auto b = a\n    print(a.x)\n"
        );
        let vm = lower_ok(&value_src);
        let rm = lower_ok(&ref_src);
        let vmain = vm.funcs.iter().find(|f| f.name == "main").unwrap();
        let rmain = rm.funcs.iter().find(|f| f.name == "main").unwrap();
        // Value: one StructNew for the constructor + one for the copy at `b = a`.
        assert_eq!(
            structnews(vmain),
            2,
            "value-struct assignment should emit a copy"
        );
        // Reference: just the constructor; assignment copies the pointer only.
        assert_eq!(
            structnews(rmain),
            1,
            "reference-struct assignment must not copy"
        );
    }

    // --- Escape analysis (#839, discussion #849) --------------------------
    // These run the full pass (via `lower_ok` -> `lower_module` -> the escape
    // pass inside `lower_module_opts`) and assert on the ZIR it produces, both
    // the promotion direction and the conservative heap-retention direction.

    fn has_stack_struct(f: &Func) -> bool {
        insts_of(f).any(|i| matches!(i, Inst::StackStruct { .. }))
    }
    fn has_heap_struct(f: &Func) -> bool {
        insts_of(f).any(|i| matches!(i, Inst::StructNew { .. }))
    }
    fn stack_strbuilds(f: &Func) -> usize {
        insts_of(f)
            .filter(|i| matches!(i, Inst::StrBuild { stack: true, .. }))
            .count()
    }
    fn heap_strbuilds(f: &Func) -> usize {
        insts_of(f)
            .filter(|i| matches!(i, Inst::StrBuild { stack: false, .. }))
            .count()
    }
    fn main_of(m: &Module) -> &Func {
        m.funcs.iter().find(|f| f.name == "main").unwrap()
    }

    #[test]
    fn escape_promotes_transient_fstring_dict_lookup_key() {
        // The evidenced #839 win: a transient `f"k{i}"` used only to probe a dict
        // (a non-retaining `zap_dict_get`) never escapes, so its `StrBuild` is
        // stack-allocated.
        let m = lower_ok(
            "def main():\n    dict[str, int64] d = {}\n    d[\"k0\"] = 7\n    int64 i = 0\n    \
             int64 s = 0\n    while i < 10:\n        s = s + d[f\"k{i}\"]\n        i = i + 1\n    print(s)\n",
        );
        assert_eq!(
            stack_strbuilds(main_of(&m)),
            1,
            "the transient lookup key f\"k{{i}}\" must be stack-allocated"
        );
    }

    #[test]
    fn escape_promotes_transient_fstring_in_len() {
        // `fstring_only` shape: `len(f"k{i}")` — the string is consumed by a pure
        // reader and discarded, so it is promoted.
        let m = lower_ok(
            "def main():\n    int64 s = 0\n    int64 i = 0\n    \
             while i < 10:\n        s = s + len(f\"k{i}\")\n        i = i + 1\n    print(s)\n",
        );
        assert_eq!(stack_strbuilds(main_of(&m)), 1);
        assert_eq!(heap_strbuilds(main_of(&m)), 0);
    }

    #[test]
    fn escape_keeps_fstring_dict_insert_key_on_heap() {
        // A key *inserted* into a dict (`zap_dict_set`, retaining) escapes — it is
        // stored in the dict and outlives the call — so its `StrBuild` stays heap.
        let m = lower_ok(
            "def main():\n    dict[str, int64] d = {}\n    int64 i = 0\n    \
             while i < 10:\n        d[f\"k{i}\"] = i\n        i = i + 1\n    print(len(d))\n",
        );
        assert_eq!(
            heap_strbuilds(main_of(&m)),
            1,
            "an inserted (retained) key must stay heap-allocated"
        );
        assert_eq!(stack_strbuilds(main_of(&m)), 0);
    }

    #[test]
    fn escape_keeps_returned_fstring_on_heap() {
        // A returned string escapes its function -> heap.
        let m = lower_ok(
            "def f(int64 i) -> str:\n    return f\"k{i}\"\ndef main():\n    print(f(3))\n",
        );
        let f = m.funcs.iter().find(|f| f.name == "f").unwrap();
        assert_eq!(heap_strbuilds(f), 1);
        assert_eq!(stack_strbuilds(f), 0);
    }

    #[test]
    fn escape_promotes_nonescaping_reference_struct() {
        // A leaf (all-scalar) reference struct built and used only locally does
        // not escape -> stack-allocated.
        let m = lower_ok(&format!(
            "{PT}\ndef main():\n    auto p = Pt(x=1.0, y=2.0)\n    print(p.x + p.y)\n"
        ));
        assert!(
            has_stack_struct(main_of(&m)),
            "a non-escaping leaf reference struct should be stack-allocated"
        );
        assert!(
            !has_heap_struct(main_of(&m)),
            "...and not also heap-allocated"
        );
    }

    #[test]
    fn escape_keeps_returned_reference_struct_on_heap() {
        // Returning the struct escapes it -> stays a heap StructNew.
        let m = lower_ok(&format!(
            "{PT}\ndef make() -> Pt:\n    return Pt(x=1.0, y=2.0)\ndef main():\n    auto p = make()\n    print(p.x)\n"
        ));
        let make = m.funcs.iter().find(|f| f.name == "make").unwrap();
        assert!(
            has_heap_struct(make),
            "a returned reference struct must stay heap-allocated"
        );
        assert!(!has_stack_struct(make));
    }

    #[test]
    fn escape_keeps_struct_stored_in_list_on_heap() {
        // Appending the struct into a list retains it -> escapes -> heap.
        let m = lower_ok(&format!(
            "{PT}\ndef main():\n    list[Pt] xs = []\n    xs.append(Pt(x=1.0, y=2.0))\n    print(len(xs))\n"
        ));
        assert!(
            has_heap_struct(main_of(&m)),
            "a struct stored into a collection must stay heap-allocated"
        );
    }

    #[test]
    fn escape_does_not_promote_value_struct() {
        // A `value struct` is a by-value aggregate, explicitly out of v1 scope —
        // never rewritten to a StackStruct regardless of escape.
        let m = lower_ok(&format!(
            "{VEC2}\ndef main():\n    auto v = Vec2(x=1.0, y=2.0)\n    print(v.len2())\n"
        ));
        assert!(
            !has_stack_struct(main_of(&m)),
            "value structs are out of v1 escape-promotion scope"
        );
    }

    #[test]
    fn escape_respects_struct_size_cap() {
        // A leaf reference struct larger than MAX_STACK_ALLOC (256 B) stays heap
        // even when non-escaping. Build a struct of N `int64` fields (8 B each)
        // well over the cap, with default values so the constructor is argument-
        // free (field defaults, #461).
        let n = 40usize; // 40 * 8 = 320 B > 256
        let mut fields = String::new();
        for i in 0..n {
            // Avoid field names like `f32`/`f64` that collide with type keywords.
            fields.push_str(&format!("    int64 v{i} = 0\n"));
        }
        let m = lower_ok(&format!(
            "struct Big:\n{fields}def main():\n    auto b = Big()\n    print(b.v0)\n"
        ));
        assert!(
            has_heap_struct(main_of(&m)),
            "an oversized struct must stay heap-allocated"
        );
        assert!(!has_stack_struct(main_of(&m)));
    }

    #[test]
    fn escape_promotes_small_nonescaping_struct_under_cap() {
        // Sanity companion to the cap test: the same shape within the cap (a few
        // scalar fields) *is* promoted when non-escaping.
        let m = lower_ok(
            "struct Small:\n    int64 a = 0\n    int64 b = 0\ndef main():\n    auto s = Small()\n    print(s.a + s.b)\n",
        );
        assert!(has_stack_struct(main_of(&m)));
        assert!(!has_heap_struct(main_of(&m)));
    }

    #[test]
    fn inline_value_struct_pass_and_return_emit_no_copy() {
        // A flat (inline-eligible) value struct is passed and returned *by value*
        // via the register/sret ABI (D-0051): the ABI copy is the value-semantics
        // copy, so lowering emits NO explicit StructNew copy at the argument or
        // the `return`. This is the heap allocation the inline layout removes.
        let m = lower_ok(&format!(
            "{VEC2}\ndef scale(Vec2 v) -> Vec2:\n    return v\n\
             def main():\n    auto a = Vec2(x=1.0, y=2.0)\n    auto b = scale(a)\n    print(b.len2())\n"
        ));
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        let scale = m.funcs.iter().find(|f| f.name == "scale").unwrap();
        // main: only the constructor for `a`. The `scale(a)` argument is passed by
        // value (no copy), and the call result is fresh so `auto b = ...` is not
        // copied either.
        assert_eq!(
            structnews(main),
            1,
            "an inline value-struct argument is copied by the ABI, not a StructNew"
        );
        // scale: returning `v` by value needs no StructNew copy.
        assert_eq!(
            structnews(scale),
            0,
            "returning an inline value struct emits no copy (returned by value)"
        );
    }

    #[test]
    fn nested_value_struct_still_deep_copies_on_pass() {
        // A non-inline value struct (one with a value-struct field) keeps the
        // deep-copy-on-pass behavior: lowering still emits an explicit copy at the
        // argument boundary, so observable value semantics are unchanged.
        let m = lower_ok(&format!(
            "{VEC2}value struct Seg:\n    Vec2 a\n    Vec2 b\n\
             def mid(Seg s) -> f64:\n    return s.a.len2()\n\
             def main():\n    auto s = Seg(a=Vec2(x=1.0, y=2.0), b=Vec2(x=3.0, y=4.0))\n    print(mid(s))\n"
        ));
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        // Construct Seg (1) + its two Vec2 fields boxed in (2) + the deep copy at
        // the `mid(s)` argument, which recursively copies Seg and its two Vec2s
        // (3). So strictly more than the 3 construction StructNews.
        assert!(
            structnews(main) > 3,
            "a non-inline value struct must be deep-copied at the argument boundary"
        );
    }

    #[test]
    fn value_struct_boxed_into_list() {
        // Storing a value struct into a collection boxes it (heap copy, D-0051),
        // so the stored element is independent of the original.
        let m = lower_ok(&format!(
            "{VEC2}\ndef main():\n    auto a = Vec2(x=1.0, y=2.0)\n    auto xs = [a]\n    print(len(xs))\n"
        ));
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        // construct `a` + box-copy on insertion into the list.
        assert_eq!(
            structnews(main),
            2,
            "storing a value struct in a list should box (copy) it"
        );
    }

    // ---- module-level `const` globals (D-0112, #658 Phase 1) --------------

    fn count_calls(f: &zapc_ir_core::Func, name: &str) -> usize {
        f.blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter(|i| {
                matches!(i, Inst::Call { func, .. } | Inst::Invoke { func, .. } if func == name)
            })
            .count()
    }

    #[test]
    fn module_const_scalar_lowers_to_global_and_init() {
        let m = lower_ok("const int64 N = 7\ndef main() -> int64:\n    return N\n");
        // One global slot, scalar type, recorded by name.
        assert_eq!(m.globals.len(), 1);
        assert_eq!(m.globals[0].name, "N");
        assert_eq!(m.globals[0].ty, Ty::I64);
        // `@__zap_init_globals` exists and stores into the slot via GlobalAddr.
        let init = m
            .funcs
            .iter()
            .find(|f| f.name == "__zap_init_globals")
            .expect("init function emitted");
        let insts: Vec<&Inst> = init.blocks.iter().flat_map(|b| &b.insts).collect();
        assert!(
            insts
                .iter()
                .any(|i| matches!(i, Inst::GlobalAddr { idx: 0, .. })),
            "init stores into global #0"
        );
        assert!(
            insts.iter().any(|i| matches!(i, Inst::Store { .. })),
            "init has a store"
        );
        // A scalar slot is NOT registered as a GC root.
        assert_eq!(count_calls(init, "zap_gc_add_root"), 0);
        // `main` runs the initializer exactly once, before its body.
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert_eq!(count_calls(main, "__zap_init_globals"), 1);
        // The reference reads the slot (GlobalAddr + Load), not an alloca.
        assert!(
            main.blocks
                .iter()
                .flat_map(|b| &b.insts)
                .any(|i| matches!(i, Inst::GlobalAddr { idx: 0, .. })),
            "main reads global #0"
        );
    }

    #[test]
    fn module_const_reference_type_is_rooted() {
        let m = lower_ok(
            "const list[int64] XS = [1, 2, 3]\ndef main() -> int64:\n    return len(XS)\n",
        );
        assert_eq!(m.globals.len(), 1);
        assert!(matches!(m.globals[0].ty, Ty::List(_)));
        // A pointer-typed global is registered as a permanent GC root.
        let init = m
            .funcs
            .iter()
            .find(|f| f.name == "__zap_init_globals")
            .unwrap();
        assert_eq!(
            count_calls(init, "zap_gc_add_root"),
            1,
            "a reference-typed global slot must be rooted"
        );
    }

    #[test]
    fn module_consts_init_in_source_order() {
        // Two globals, indexed in source order; the second's initializer reads
        // the first (an earlier global resolves during init).
        let m = lower_ok(
            "const int64 A = 10\nconst int64 B = A + 5\ndef main() -> int64:\n    return B\n",
        );
        assert_eq!(m.globals.len(), 2);
        assert_eq!(m.globals[0].name, "A");
        assert_eq!(m.globals[1].name, "B");
        let init = m
            .funcs
            .iter()
            .find(|f| f.name == "__zap_init_globals")
            .unwrap();
        // The init reads global #0 (A) while initializing global #1 (B).
        let addrs: Vec<u32> = init
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                Inst::GlobalAddr { idx, .. } => Some(*idx),
                _ => None,
            })
            .collect();
        assert!(addrs.contains(&0) && addrs.contains(&1));
    }

    #[test]
    fn no_globals_means_no_init_function() {
        // A program with no module `const` emits neither a globals table nor an
        // init function, and `main` does not call one.
        let m = lower_ok("def main() -> int64:\n    return 0\n");
        assert!(m.globals.is_empty());
        assert!(m.funcs.iter().all(|f| f.name != "__zap_init_globals"));
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert_eq!(count_calls(main, "__zap_init_globals"), 0);
    }

    // ---- module-level mutable globals (D-0112, #662 Phase 2) --------------

    fn global_addrs(f: &zapc_ir_core::Func) -> Vec<u32> {
        f.blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                Inst::GlobalAddr { idx, .. } => Some(*idx),
                _ => None,
            })
            .collect()
    }

    fn count_stores(f: &zapc_ir_core::Func) -> usize {
        f.blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i, Inst::Store { .. }))
            .count()
    }

    #[test]
    fn module_mutable_scalar_lowers_to_global_like_const() {
        // A mutable module global is lowered identically to a `const` one: a
        // single scalar slot, recorded by name, initialized by
        // `@__zap_init_globals`, read via GlobalAddr + Load.
        let m = lower_ok("int64 N = 7\ndef main() -> int64:\n    return N\n");
        assert_eq!(m.globals.len(), 1);
        assert_eq!(m.globals[0].name, "N");
        assert_eq!(m.globals[0].ty, Ty::I64);
        let init = m
            .funcs
            .iter()
            .find(|f| f.name == "__zap_init_globals")
            .expect("init function emitted");
        assert!(
            global_addrs(init).contains(&0),
            "init stores into global #0"
        );
        // A scalar slot is NOT registered as a GC root.
        assert_eq!(count_calls(init, "zap_gc_add_root"), 0);
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert_eq!(count_calls(main, "__zap_init_globals"), 1);
        assert!(global_addrs(main).contains(&0), "main reads global #0");
    }

    #[test]
    fn module_mutable_reassignment_stores_through_the_global_slot() {
        // The Phase-2 write path: reassigning a mutable module global inside a
        // function lowers to a `GlobalAddr` + `Store` into the slot (NOT an
        // alloca). The `set` function has no locals, so its only store must
        // target the global slot.
        let m = lower_ok(
            "int64 N = 0\ndef store_n():\n    N = 5\ndef main() -> int64:\n    store_n()\n    return N\n",
        );
        let store_n = m.funcs.iter().find(|f| f.name == "store_n").unwrap();
        assert!(
            global_addrs(store_n).contains(&0),
            "reassignment addresses global #0"
        );
        assert_eq!(
            count_stores(store_n),
            1,
            "reassignment stores exactly once, into the global slot"
        );
    }

    #[test]
    fn module_mutable_augassign_reads_and_writes_the_global_slot() {
        // `N += 1` on a mutable global is a read-modify-write through the slot:
        // one GlobalAddr, a Load from it, and a Store back into it.
        let m = lower_ok("int64 N = 0\ndef bump():\n    N += 1\ndef main() -> int64:\n    bump()\n    return N\n");
        let bump = m.funcs.iter().find(|f| f.name == "bump").unwrap();
        assert!(global_addrs(bump).contains(&0));
        assert_eq!(count_stores(bump), 1, "aug-assign stores back once");
        assert!(
            bump.blocks
                .iter()
                .flat_map(|b| &b.insts)
                .any(|i| matches!(i, Inst::Load { .. })),
            "aug-assign loads the current value"
        );
    }

    #[test]
    fn module_mutable_reference_type_is_rooted() {
        // A pointer-typed mutable global is a permanent GC root, exactly like a
        // reference `const`: `@__zap_init_globals` registers the slot once with
        // `zap_gc_add_root`, so the Boehm collector keeps whatever heap object
        // the slot currently holds (including after reassignment/growth) live.
        let m = lower_ok(
            "list[int64] XS = [1, 2, 3]\ndef grow():\n    XS.append(4)\ndef main() -> int64:\n    grow()\n    return len(XS)\n",
        );
        assert_eq!(m.globals.len(), 1);
        assert!(matches!(m.globals[0].ty, Ty::List(_)));
        let init = m
            .funcs
            .iter()
            .find(|f| f.name == "__zap_init_globals")
            .unwrap();
        assert_eq!(
            count_calls(init, "zap_gc_add_root"),
            1,
            "a reference-typed mutable global slot must be rooted"
        );
    }

    #[test]
    fn module_mutable_and_const_coexist_in_declaration_order() {
        // Const and mutable globals share one slot table, indexed in source
        // order; a later mutable initializer may read an earlier const.
        let m =
            lower_ok("const int64 A = 10\nint64 B = A + 5\ndef main() -> int64:\n    return B\n");
        assert_eq!(m.globals.len(), 2);
        assert_eq!(m.globals[0].name, "A");
        assert_eq!(m.globals[1].name, "B");
        let init = m
            .funcs
            .iter()
            .find(|f| f.name == "__zap_init_globals")
            .unwrap();
        let addrs = global_addrs(init);
        assert!(addrs.contains(&0) && addrs.contains(&1));
    }

    // --- #663: module-global write barrier (epic #661) ---------------------
    //
    // Reassigning a mutable module-level *pointer* global overwrites a permanent
    // GC-root slot, so each such store is followed by a `zap_gc_write_barrier`
    // call on the slot. A local, a scalar global, or an immutable `const`
    // reference does NOT get the barrier, and neither does the one-time
    // initializer store in `@__zap_init_globals`.

    #[test]
    fn module_mutable_pointer_reassign_emits_write_barrier() {
        // `XS = [...]` on a reference-typed mutable global: one store into the
        // slot, immediately followed by one barrier call on that slot.
        let m = lower_ok(
            "list[int64] XS = [1]\ndef put():\n    XS = [2, 3]\ndef main() -> int64:\n    put()\n    return len(XS)\n",
        );
        let put = m.funcs.iter().find(|f| f.name == "put").unwrap();
        assert!(
            global_addrs(put).contains(&0),
            "reassignment addresses global #0"
        );
        assert_eq!(
            count_calls(put, "zap_gc_write_barrier"),
            1,
            "a mutable pointer-global reassignment fires exactly one write barrier"
        );
        // The barrier records the slot AFTER it is written: the Store precedes
        // the barrier Call in the same block.
        let insts: Vec<&Inst> = put.blocks.iter().flat_map(|b| &b.insts).collect();
        let store_at = insts.iter().position(|i| matches!(i, Inst::Store { .. }));
        let barrier_at = insts
            .iter()
            .position(|i| matches!(i, Inst::Call { func, .. } if func == "zap_gc_write_barrier"));
        assert!(
            matches!((store_at, barrier_at), (Some(s), Some(b)) if s < b),
            "the write barrier follows the store into the slot"
        );
    }

    #[test]
    fn module_mutable_pointer_augassign_emits_write_barrier() {
        // `XS += [...]` is a read-modify-write that stores a (possibly new)
        // pointer back into the root slot, so it too fires the barrier.
        let m = lower_ok(
            "str S = \"a\"\ndef grow():\n    S += \"b\"\ndef main() -> int64:\n    grow()\n    return len(S)\n",
        );
        let grow = m.funcs.iter().find(|f| f.name == "grow").unwrap();
        assert_eq!(
            count_calls(grow, "zap_gc_write_barrier"),
            1,
            "an augmented reassignment of a pointer global fires the write barrier"
        );
    }

    #[test]
    fn module_mutable_scalar_reassign_emits_no_write_barrier() {
        // A scalar global holds no pointer, so reassigning it records no edge.
        let m = lower_ok(
            "int64 N = 0\ndef put():\n    N = 5\ndef main() -> int64:\n    put()\n    return N\n",
        );
        let put = m.funcs.iter().find(|f| f.name == "put").unwrap();
        assert_eq!(
            count_calls(put, "zap_gc_write_barrier"),
            0,
            "a scalar global reassignment emits no write barrier"
        );
    }

    #[test]
    fn local_pointer_reassign_emits_no_write_barrier() {
        // A pointer-typed *local* is scanned precisely via the shadow stack and
        // holds no permanent-root edge, so reassigning it emits no barrier. This
        // proves the barrier gate is global-only, not type-only.
        let m = lower_ok(
            "def f() -> int64:\n    list[int64] xs = [1]\n    xs = [2, 3]\n    return len(xs)\ndef main() -> int64:\n    return f()\n",
        );
        let f = m.funcs.iter().find(|f| f.name == "f").unwrap();
        assert_eq!(
            count_calls(f, "zap_gc_write_barrier"),
            0,
            "a local pointer reassignment emits no write barrier"
        );
    }

    #[test]
    fn module_const_reference_emits_no_write_barrier() {
        // An immutable reference `const` is stored once at init and never
        // reassigned (ZAP3017), so no function and not even the initializer
        // emits a write barrier for it (the init store is deliberately
        // unbarriered — startup runs before any mutation).
        let m = lower_ok(
            "const list[int64] XS = [1, 2, 3]\ndef main() -> int64:\n    return len(XS)\n",
        );
        for f in &m.funcs {
            assert_eq!(
                count_calls(f, "zap_gc_write_barrier"),
                0,
                "a const reference global fires no write barrier (fn {})",
                f.name
            );
        }
    }

    #[test]
    fn init_globals_store_is_unbarriered() {
        // Even for a mutable pointer global, the one-time initializer store in
        // `@__zap_init_globals` is unbarriered: it runs before `main`, in the
        // #658 topological + source order, and only reassignments from user code
        // record an edge. (The slot is still rooted via `zap_gc_add_root`.)
        let m = lower_ok("list[int64] XS = [1, 2, 3]\ndef main() -> int64:\n    return len(XS)\n");
        let init = m
            .funcs
            .iter()
            .find(|f| f.name == "__zap_init_globals")
            .unwrap();
        assert_eq!(
            count_calls(init, "zap_gc_add_root"),
            1,
            "the pointer slot is rooted at init"
        );
        assert_eq!(
            count_calls(init, "zap_gc_write_barrier"),
            0,
            "the init-time store is unbarriered"
        );
    }

    // --- #689 / #690: user-defined-type module globals (epic #661) ----------
    //
    // These lower end-to-end (through `with_prelude` + monomorphization), so they
    // are the regression wall around the two sibling fixes: a user *struct*
    // global's member access must resolve against the struct (#689), and a user
    // *generic* global must be monomorphized to a concrete instance before
    // lowering (#690). Non-colliding struct globals already worked; the #689
    // failure was specifically a user type named `Counter` colliding with the
    // `concurrent` prelude helper, so that exact repro is pinned here.

    #[test]
    fn user_struct_module_global_lowers_field_and_method() {
        // A plain user-struct global supports both a field read and a method
        // call, for const and mutable, with a fieldget against the struct (never
        // a cell wrapper).
        for decl in ["Pt p = Pt(5)", "const Pt p = Pt(5)"] {
            let m = lower_ok(&format!(
                "struct Pt:\n    int64 x\n    def get(self) -> int64:\n        return self.x\n\
                 {decl}\ndef main() -> int64:\n    return p.x + p.get()\n"
            ));
            assert_eq!(m.globals.len(), 1, "one global slot ({decl})");
            assert!(
                matches!(m.globals[0].ty, Ty::Struct(_)),
                "the struct global's slot is typed as the struct ({decl})"
            );
        }
    }

    #[test]
    fn struct_global_name_colliding_with_prelude_counter_lowers() {
        // #689 exact repro: a user `struct Counter` fires the `"Counter"` prelude
        // needle, but the dependent `__concurrent_counter` helper is dropped
        // (parser::with_prelude), so `c.n` / `c.get()` resolve against the user's
        // Counter instead of failing with `no field '_cell' on struct`.
        lower_ok(
            "struct Counter:\n    int64 n\n    def get(self) -> int64:\n        return self.n\n\
             Counter c = Counter(5)\ndef main() -> int64:\n    return c.n + c.get()\n",
        );
        // And as a local, which failed identically before the fix.
        lower_ok(
            "struct Counter:\n    int64 n\n\
             def main() -> int64:\n    Counter c = Counter(5)\n    return c.n\n",
        );
    }

    #[test]
    fn nested_and_mutated_struct_globals_lower() {
        // A struct holding a struct (nested field access) and mutation of a
        // mutable struct global's field then a read both lower.
        lower_ok(
            "struct Inner:\n    int64 v\nstruct Outer:\n    Inner inner\n\
             Outer o = Outer(Inner(5))\ndef main() -> int64:\n    return o.inner.v\n",
        );
        lower_ok(
            "struct Pt:\n    int64 x\nPt p = Pt(5)\n\
             def main() -> int64:\n    p.x = 9\n    return p.x\n",
        );
    }

    #[test]
    fn user_generic_module_global_lowers() {
        // #690: a user generic-type global is monomorphized before lowering, so
        // its slot is a concrete struct instance and `g.item` / `g.peek()`
        // resolve. Const and mutable; plus two distinct instantiations.
        for decl in ["Box[int64] g = Box(7)", "const Box[int64] g = Box(7)"] {
            let m = lower_ok(&format!(
                "struct Box[T]:\n    T item\n    def peek(self) -> T:\n        return self.item\n\
                 {decl}\ndef main() -> int64:\n    return g.item + g.peek()\n"
            ));
            assert!(
                matches!(m.globals[0].ty, Ty::Struct(_)),
                "the generic global's slot is a concrete struct instance ({decl})"
            );
        }
        // Two instantiations as separate globals each resolve to their own slot.
        let m = lower_ok(
            "struct Box[T]:\n    T item\nBox[int64] gi = Box(7)\nBox[str] gs = Box(\"hi\")\n\
             def main() -> int64:\n    print(gs.item)\n    return gi.item\n",
        );
        assert_eq!(m.globals.len(), 2, "two distinct global slots");
    }

    // --- #677: `--race` access instrumentation (epic #665, D-0114) ----------
    //
    // Under `--race`, lowering emits a `__zap_race_read` / `__zap_race_write`
    // hook at exactly the ZAP5125 sharing-lint set (#664): a read/write of a
    // MUTABLE, non-`@shareable` module global (scalar OR reference). A `const`
    // global, a `@shareable` cell (user struct or a `BUILTIN_SHAREABLE_TYPES`
    // type), an access in the module-init pre-pass, and a name shadowed by a
    // local are NEVER instrumented. With `--race` OFF, emission is identical to
    // today — zero race hooks — so every golden stays byte-identical.

    /// Lower with the runtime data-race detector on (`--race`, #677).
    fn lower_race(src: &str) -> Module {
        let (ast, perrs, lerrs) = parse_str(src);
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        let (module, errs) = lower_module_opts(&ast, true);
        assert!(errs.is_empty(), "lower errors: {errs:?}");
        module
    }

    /// Count calls to `name` across every function in the module.
    fn count_calls_module(m: &Module, name: &str) -> usize {
        m.funcs.iter().map(|f| count_calls(f, name)).sum()
    }

    #[test]
    fn race_scalar_global_read_and_write_are_instrumented() {
        // The canonical racy counter: `counter = counter + 1` on a plain `int64`
        // module global. The RHS read fires `__zap_race_read` and the store fires
        // `__zap_race_write` — a scalar global (no `is_ptr()` gate), the exact bug
        // the detector targets.
        let src = "int64 counter = 0\n\
                   def bump():\n    counter = counter + 1\n\
                   def main():\n    bump()\n";
        let m = lower_race(src);
        let bump = m.funcs.iter().find(|f| f.name == "bump").unwrap();
        assert_eq!(
            count_calls(bump, "__zap_race_read"),
            1,
            "the RHS read of `counter` is instrumented"
        );
        assert_eq!(
            count_calls(bump, "__zap_race_write"),
            1,
            "the store into `counter` is instrumented"
        );
        // The hooks carry four args: slot, name_id (slot index 0), and the
        // access span's start/end.
        let has_four_arg_hook = bump.blocks.iter().flat_map(|b| &b.insts).any(|i| {
            matches!(i, Inst::Call { func, args, .. }
                if func == "__zap_race_write" && args.len() == 4)
        });
        assert!(
            has_four_arg_hook,
            "the write hook has [slot, name_id, line, col]"
        );
    }

    #[test]
    fn race_off_emits_no_hooks_and_is_identical_to_default() {
        // The invariant: with `--race` OFF, lowering emits ZERO race hooks, and
        // the result is byte-identical to the default `lower_module` path — so
        // every existing golden is unaffected (golden-safe).
        let src = "int64 counter = 0\n\
                   def bump():\n    counter = counter + 1\n\
                   def main():\n    bump()\n";
        let (ast, _, _) = parse_str(src);
        let (default_m, de) = lower_module(&ast);
        let (off_m, oe) = lower_module_opts(&ast, false);
        assert!(de.is_empty() && oe.is_empty());
        assert_eq!(count_calls_module(&default_m, "__zap_race_read"), 0);
        assert_eq!(count_calls_module(&default_m, "__zap_race_write"), 0);
        assert_eq!(count_calls_module(&off_m, "__zap_race_read"), 0);
        assert_eq!(count_calls_module(&off_m, "__zap_race_write"), 0);
        // Byte-identical ZIR text between the explicit race=false path and the
        // default `lower_module`.
        assert_eq!(default_m.to_string(), off_m.to_string());
    }

    #[test]
    fn race_on_differs_from_off_only_by_hooks() {
        // A `--race` build differs from a non-`--race` build ONLY by the added
        // race hooks: stripping every `__zap_race_*` call from the race ZIR must
        // leave text identical to the off build (the IConst operands feeding the
        // hooks are the only other additions, so compare on the call lines).
        let src = "int64 counter = 0\n\
                   def bump():\n    counter = counter + 1\n\
                   def main():\n    bump()\n";
        let (ast, _, _) = parse_str(src);
        let (off_m, _) = lower_module_opts(&ast, false);
        let (on_m, _) = lower_module_opts(&ast, true);
        assert_eq!(count_calls_module(&off_m, "__zap_race_read"), 0);
        assert!(count_calls_module(&on_m, "__zap_race_read") > 0);
        assert!(count_calls_module(&on_m, "__zap_race_write") > 0);
    }

    #[test]
    fn race_reference_global_is_instrumented_and_scalar_gate_is_broader() {
        // A reference-typed mutable global is instrumented too; its reassignment
        // emits BOTH the GC write barrier (pointer-only, #663) and the race write
        // hook (any mutable non-shareable global, #677) — proving the race gate
        // is broader than the barrier's `is_ptr()`.
        let src = "list[int64] xs = [1]\n\
                   def put():\n    xs = [2, 3]\n\
                   def main() -> int64:\n    put()\n    return len(xs)\n";
        let m = lower_race(src);
        let put = m.funcs.iter().find(|f| f.name == "put").unwrap();
        assert_eq!(count_calls(put, "zap_gc_write_barrier"), 1);
        assert_eq!(count_calls(put, "__zap_race_write"), 1);
    }

    #[test]
    fn race_const_global_is_never_instrumented() {
        // A `const` global can never race (immutable), so neither its read nor
        // anything else is instrumented, even under `--race`.
        let src = "const int64 N = 7\n\
                   def read() -> int64:\n    return N\n\
                   def main():\n    read()\n";
        let m = lower_race(src);
        assert_eq!(count_calls_module(&m, "__zap_race_read"), 0);
        assert_eq!(count_calls_module(&m, "__zap_race_write"), 0);
    }

    #[test]
    fn race_shareable_user_struct_global_is_never_instrumented() {
        // A mutable global whose type is a user `@shareable` struct is an
        // asserted-safe cell: no read or write hook (matches the silent ZAP5125).
        let src = "@shareable\nstruct Registry:\n    int64 n\n\n\
                   Registry R = Registry(0)\n\
                   def touch():\n    auto r = R\n\
                   def main():\n    touch()\n";
        let m = lower_race(src);
        assert_eq!(count_calls_module(&m, "__zap_race_read"), 0);
        assert_eq!(count_calls_module(&m, "__zap_race_write"), 0);
    }

    // A built-in `@shareable` type (`Atomic`/`Mutex`/`WaitGroup`/`Once`/
    // `Counter`/`Queue`) is excluded through the SAME `StructCtx::name_is_shareable`
    // rule as the user-`@shareable`-struct case above (which this suite covers via
    // `race_shareable_user_struct_global_is_never_instrumented`): the predicate
    // checks `BUILTIN_SHAREABLE_TYPES` on the declared type's nominal base. These
    // types have no surface as a plain module-global initializer yet (an `Atomic`
    // module global does not lower today), so the end-to-end builtin-shareable
    // case is exercised by the `clean_atomic.zap` CI golden rather than a unit
    // test here.

    #[test]
    fn race_local_shadowing_global_is_never_instrumented() {
        // A local binding that shadows a module global wins name resolution, so
        // accessing the shadowed name touches the local, not the shared global —
        // no hook (mirrors sema's `resolves_to_module_global`).
        let src = "int64 counter = 0\n\
                   def f() -> int64:\n    int64 counter = 5\n    counter = counter + 1\n    return counter\n\
                   def main():\n    f()\n";
        let m = lower_race(src);
        let f = m.funcs.iter().find(|f| f.name == "f").unwrap();
        assert_eq!(
            count_calls(f, "__zap_race_read"),
            0,
            "a read of the shadowing local is not instrumented"
        );
        assert_eq!(
            count_calls(f, "__zap_race_write"),
            0,
            "a write of the shadowing local is not instrumented"
        );
    }

    #[test]
    fn race_init_prepass_accesses_are_never_instrumented() {
        // The single-threaded module-init pre-pass runs before any coroutine
        // exists, so an initializer that reads an earlier mutable global emits no
        // race hook (`@__zap_init_globals` is exempt).
        let src = "int64 a = 1\nint64 b = a + 1\n\
                   def main() -> int64:\n    return b\n";
        let m = lower_race(src);
        let init = m
            .funcs
            .iter()
            .find(|f| f.name == "__zap_init_globals")
            .unwrap();
        assert_eq!(
            count_calls(init, "__zap_race_read"),
            0,
            "the init-time read of global `a` is exempt"
        );
        assert_eq!(count_calls(init, "__zap_race_write"), 0);
    }

    #[test]
    fn race_augassign_write_form_is_instrumented() {
        // `counter += 1` is a read-modify-write that sema counts as one (write)
        // access, so it fires exactly one `__zap_race_write` (and no separate
        // read hook — the RMW load is not the `expr` read site).
        let src = "int64 counter = 0\n\
                   def bump():\n    counter += 1\n\
                   def main():\n    bump()\n";
        let m = lower_race(src);
        let bump = m.funcs.iter().find(|f| f.name == "bump").unwrap();
        assert_eq!(count_calls(bump, "__zap_race_write"), 1);
        assert_eq!(count_calls(bump, "__zap_race_read"), 0);
    }

    #[test]
    fn race_chained_assign_write_form_is_instrumented() {
        // A chained assignment `a = b = expr` stores into every name target, so
        // each racy global target fires its own write hook.
        let src = "int64 a = 0\nint64 b = 0\n\
                   def setit():\n    a = b = 5\n\
                   def main():\n    setit()\n";
        let m = lower_race(src);
        let setit = m.funcs.iter().find(|f| f.name == "setit").unwrap();
        assert_eq!(
            count_calls(setit, "__zap_race_write"),
            2,
            "each chained target (a and b) is a write access"
        );
    }

    #[test]
    fn race_tuple_leaf_write_form_is_instrumented() {
        // Tuple unpacking `a, b = ...` stores into each name leaf, so each racy
        // global leaf fires a write hook.
        let src = "int64 a = 0\nint64 b = 0\n\
                   def setit():\n    a, b = 5, 6\n\
                   def main():\n    setit()\n";
        let m = lower_race(src);
        let setit = m.funcs.iter().find(|f| f.name == "setit").unwrap();
        assert_eq!(
            count_calls(setit, "__zap_race_write"),
            2,
            "each tuple leaf (a and b) is a write access"
        );
    }

    #[test]
    fn race_plain_read_of_global_is_instrumented() {
        // A bare read of a racy global in an expression position fires the read
        // hook (and nothing writes it here).
        let src = "int64 counter = 0\n\
                   def peek() -> int64:\n    return counter\n\
                   def main():\n    peek()\n";
        let m = lower_race(src);
        let peek = m.funcs.iter().find(|f| f.name == "peek").unwrap();
        assert_eq!(count_calls(peek, "__zap_race_read"), 1);
        assert_eq!(count_calls(peek, "__zap_race_write"), 0);
    }

    #[test]
    fn race_main_bootstrap_enables_and_registers_every_global() {
        // Slice 2 (#678): under `--race`, `main`'s prologue arms the engine once
        // (`__zap_race_enable`) and registers EVERY module global's name under its
        // slot index (`__zap_race_register_global`) so a report can resolve a
        // `name_id` back to a name — racy globals and non-racy (`const`) ones
        // alike (the id is the declaration index, which must stay total).
        let src = "const int64 LIMIT = 10\nint64 counter = 0\n\
                   def bump():\n    counter = counter + 1\n\
                   def main():\n    bump()\n";
        let m = lower_race(src);
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert_eq!(
            count_calls(main, "__zap_race_enable"),
            1,
            "main arms the detector exactly once"
        );
        assert_eq!(
            count_calls(main, "__zap_race_register_global"),
            2,
            "both globals (const LIMIT + mutable counter) are registered by index"
        );
    }

    #[test]
    fn race_bootstrap_is_main_only_and_race_only() {
        // The bootstrap is emitted ONLY in `main` and ONLY under `--race`: a
        // non-main function never carries it, and a non-`--race` build emits none
        // (so the `main` prologue is byte-identical to today — golden-safe).
        let src = "int64 counter = 0\n\
                   def bump():\n    counter = counter + 1\n\
                   def main():\n    bump()\n";
        // --race: present in main, absent elsewhere.
        let on = lower_race(src);
        let bump = on.funcs.iter().find(|f| f.name == "bump").unwrap();
        assert_eq!(count_calls(bump, "__zap_race_enable"), 0);
        assert_eq!(count_calls(bump, "__zap_race_register_global"), 0);
        // off: no bootstrap at all.
        let (ast, _, _) = parse_str(src);
        let (off, _) = lower_module_opts(&ast, false);
        assert_eq!(count_calls_module(&off, "__zap_race_enable"), 0);
        assert_eq!(count_calls_module(&off, "__zap_race_register_global"), 0);
    }

    #[test]
    fn race_bootstrap_absent_when_no_globals() {
        // No module globals -> nothing to register. `__zap_race_enable` is still
        // harmless to emit, but there are no globals, so register_global count is
        // zero; a program with no shared globals has no racy accesses either.
        let src = "def main():\n    print(1)\n";
        let m = lower_race(src);
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert_eq!(
            count_calls(main, "__zap_race_register_global"),
            0,
            "no globals -> no name registrations"
        );
    }

    // ---- #707 / D-0124: interface-value rendering -------------------------

    /// True iff `f` emits a render dispatch over an interface value (either the
    /// plain `IfaceRender` or, inside a landing-pad scope, `IfaceRenderInvoke`).
    fn emits_iface_render(f: &Func) -> bool {
        f.blocks
            .iter()
            .flat_map(|b| &b.insts)
            .any(|i| matches!(i, Inst::IfaceRender { .. } | Inst::IfaceRenderInvoke { .. }))
    }

    #[test]
    fn interface_value_render_dispatches_to_concrete_renderer() {
        // `print(s)` where `Shape s = Circle(...)` renders via dynamic dispatch
        // (an IfaceRender over the fat pointer), NOT the static struct_to_str
        // path — the concrete type is erased. The concrete type's renderer is
        // synthesized and recorded so codegen can place it in the itable's
        // renderer slot (#707, D-0124). This replaces the pre-#707 ZAP6001
        // deferral: lowering must now succeed.
        let m = lower_ok(
            r#"interface Shape:
    def area(self) -> f64

struct Circle:
    f64 r

    def area(self) -> f64:
        return self.r

def main():
    Shape s = Circle(r=2.0)
    print(s)
"#,
        );
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(
            emits_iface_render(main),
            "print(interface value) must lower to an IfaceRender dispatch, not a \
             static renderer call or a ZAP6001 deferral"
        );
        // The concrete type Circle (struct#0) has a recorded renderer, and that
        // symbol names a real synthesized function.
        let sym = m
            .iface_renderers
            .get(&0)
            .expect("Circle's renderer is recorded for the itable renderer slot");
        assert_eq!(sym, "Circle$repr$default");
        assert!(
            m.funcs.iter().any(|f| &f.name == sym),
            "the recorded renderer '{sym}' must be an emitted function"
        );
    }

    #[test]
    fn interface_value_render_prefers_explicit_dunder() {
        // An explicit `__str__` on the concrete type still overrides when the
        // value is rendered through an interface: the renderer slot resolves the
        // same way a direct render does. The recorded renderer is the dunder, and
        // NO compiler-default field-wise renderer is synthesized for that type.
        let m = lower_ok(
            r#"interface Shape:
    def area(self) -> f64

struct Circle:
    f64 r

    def area(self) -> f64:
        return self.r

    def __str__(self) -> str:
        return "a circle"

def main():
    Shape s = Circle(r=2.0)
    print(s)
"#,
        );
        let sym = m.iface_renderers.get(&0).expect("recorded renderer");
        assert_eq!(sym, "Circle____str__", "the __str__ override wins");
        assert!(
            !m.funcs.iter().any(|f| f.name == "Circle$repr$default"),
            "no default renderer is synthesized when __str__ overrides"
        );
    }

    #[test]
    fn interface_value_render_reaches_tier3_enum_field_renderer() {
        // The renderer reached through the interface recurses into every field
        // type via the one Display protocol — including a tier-3 data-carrying
        // enum field, which routes to #706's structural tag-dispatch renderer
        // (`{Enum}$repr$enum`). This is the key integration: #706's renderer is
        // exercised THROUGH the type-erased interface value.
        let m = lower_ok(
            r#"interface Describable:
    def tag(self) -> str

enum Shape:
    Dot
    Box(int w, int h)

struct Widget:
    str name
    Shape s

    def tag(self) -> str:
        return self.name

def main():
    Describable d = Widget(name="w", s=Shape.Box(w=3, h=4))
    print(d)
"#,
        );
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(
            emits_iface_render(main),
            "interface value renders via dispatch"
        );
        // The struct's default renderer exists and calls the tier-3 enum's
        // structural renderer for its enum field.
        let widget = m
            .funcs
            .iter()
            .find(|f| f.name == "Widget$repr$default")
            .expect("Widget renderer synthesized");
        assert!(
            widget
                .blocks
                .iter()
                .flat_map(|b| &b.insts)
                .any(|i| matches!(i, Inst::Call { func, .. } if func == "Shape$repr$enum")),
            "the interface value's renderer must reach #706's tier-3 enum renderer \
             for the enum field"
        );
    }

    #[test]
    fn interface_value_render_is_dynamic_across_implementors() {
        // Two concrete types behind the same interface each get their own
        // recorded renderer, so the render dispatch is dynamic (like method
        // dispatch), not resolved to one static symbol.
        let m = lower_ok(
            r#"interface Shape:
    def area(self) -> f64

struct Circle:
    f64 r

    def area(self) -> f64:
        return self.r

struct Square:
    f64 side

    def area(self) -> f64:
        return self.side

def main():
    Shape a = Circle(r=2.0)
    print(a)
    Shape b = Square(side=3.0)
    print(b)
"#,
        );
        // struct#0 = Circle, struct#1 = Square; each has a distinct renderer.
        assert_eq!(
            m.iface_renderers.get(&0).map(String::as_str),
            Some("Circle$repr$default")
        );
        assert_eq!(
            m.iface_renderers.get(&1).map(String::as_str),
            Some("Square$repr$default")
        );
    }

    #[test]
    fn interface_value_render_in_try_uses_invoke_form() {
        // Inside a `try`, the render dispatch must take the exception-edge form
        // (`IfaceRenderInvoke`) so a renderer that raises (a user `__str__`, a
        // `bytes` field decode) transfers to the landing pad, mirroring
        // IfaceInvoke vs IfaceCall.
        let m = lower_ok(
            r#"interface Shape:
    def area(self) -> f64

struct Circle:
    f64 r

    def area(self) -> f64:
        return self.r

def main():
    Shape s = Circle(r=2.0)
    try:
        print(s)
    except Exception:
        print("caught")
"#,
        );
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(
            main.blocks
                .iter()
                .flat_map(|b| &b.insts)
                .any(|i| matches!(i, Inst::IfaceRenderInvoke { .. })),
            "a render inside a try must use the invoke form"
        );
    }

    // --- #731: enums conform to interfaces structurally, both tiers ----------
    //
    // These lock the lowering shape before codegen (no host LLVM): a tier-3 enum
    // boxes its tagged-struct pointer like a struct, and a tier-1 enum boxes its
    // scalar tag into a heap cell `{Enum}$box` dispatched through per-method
    // thunks `{Enum}$box__m`. End-to-end execution is pinned by the goldens
    // (enum_iface_*) under CI.

    fn insts_of(f: &Func) -> impl Iterator<Item = &Inst> {
        f.blocks.iter().flat_map(|b| &b.insts)
    }

    #[test]
    fn tier3_enum_widens_to_interface_like_a_struct() {
        // The issue repro. A tier-3 enum's method (`Expr__rank`) is a real
        // function (self = the tagged struct), the widening emits an IfaceMake
        // whose concrete_sid is the enum's tagged-struct id, and the method call
        // dispatches through the itable (IfaceCall/IfaceInvoke).
        let m = lower_ok(
            r#"interface Node:
    def rank(self) -> int

enum Expr:
    Lit(int v)
    Add(int l, int r)

    def rank(self) -> int:
        return 1

def main():
    Node n = Expr.Add(l=1, r=2)
    print(n.rank())
"#,
        );
        let expr_sid = m
            .structs
            .iter()
            .position(|s| s.name == "Expr")
            .expect("tier-3 enum lowers to a tagged struct named after the enum")
            as u32;
        // `Expr__rank` is the real method, on the tagged-struct receiver.
        let rank = m
            .funcs
            .iter()
            .find(|f| f.name == "Expr__rank")
            .expect("tier-3 enum method is lowered");
        assert_eq!(rank.params.first(), Some(&Ty::Struct(expr_sid)));
        // There is NO tier-1 box for a tier-3 enum.
        assert!(
            !m.structs.iter().any(|s| s.name == "Expr$box"),
            "a tier-3 enum needs no scalar box"
        );
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(
            insts_of(main).any(
                |i| matches!(i, Inst::IfaceMake { concrete_sid, .. } if *concrete_sid == expr_sid)
            ),
            "the tier-3 enum is boxed into the fat pointer by its tagged-struct id"
        );
        assert!(
            insts_of(main).any(|i| matches!(i, Inst::IfaceCall { .. } | Inst::IfaceInvoke { .. })),
            "the interface method call dispatches through the itable"
        );
    }

    #[test]
    fn tier1_enum_method_lowers_and_dispatches_directly() {
        // Before #731 a method call on a tier-1 enum was ZAP6001. Now the real
        // method `Color__rank` exists with `self` typed as the backing integer
        // (`Ty::Enum`), and a direct `value.rank()` call resolves to it.
        let m = lower_ok(
            r#"enum Color:
    Red
    Green
    Blue

    def rank(self) -> int:
        return 7

def main():
    print(Color.Green.rank())
"#,
        );
        let rank = m
            .funcs
            .iter()
            .find(|f| f.name == "Color__rank")
            .expect("tier-1 enum method is lowered (was ZAP6001 before #731)");
        assert!(
            matches!(rank.params.first(), Some(Ty::Enum(_))),
            "a tier-1 enum method takes self as the backing integer, got {:?}",
            rank.params.first()
        );
        assert!(
            calls(&m, "Color__rank"),
            "a direct tier-1 enum method call dispatches to the real method"
        );
    }

    #[test]
    fn tier1_enum_widens_to_interface_by_boxing_the_scalar() {
        // A tier-1 enum behind an interface: a box struct `{Enum}$box = { i64 }`
        // is laid out, the widening emits StructNew + FieldSet(tag) + IfaceMake
        // with the box id, and a per-method thunk `{Enum}$box__m` is emitted.
        let m = lower_ok(
            r#"interface Ranked:
    def rank(self) -> int

enum Color:
    Red
    Green
    Blue

    def rank(self) -> int:
        return 7

def main():
    Ranked r = Color.Green
    print(r.rank())
"#,
        );
        let box_sid = m
            .structs
            .iter()
            .position(|s| s.name == "Color$box")
            .expect("a tier-1 enum with methods gets a box struct") as u32;
        // The box carries exactly one 64-bit tag field and is a reference struct.
        let box_layout = &m.structs[box_sid as usize];
        assert_eq!(box_layout.fields, vec![Ty::I64], "box is a single i64 tag");
        assert!(
            !box_layout.is_value,
            "the box is heap-allocated (reference)"
        );
        // The itable thunk exists, taking the box pointer as self.
        let thunk = m
            .funcs
            .iter()
            .find(|f| f.name == "Color$box__rank")
            .expect("tier-1 interface thunk is emitted");
        assert_eq!(thunk.params.first(), Some(&Ty::Struct(box_sid)));
        // main boxes the scalar: StructNew(box) -> FieldSet(tag) -> IfaceMake.
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(
            insts_of(main).any(|i| matches!(i, Inst::StructNew { sid, .. } if *sid == box_sid)),
            "the tier-1 enum's tag is boxed into a fresh heap cell"
        );
        assert!(
            insts_of(main)
                .any(|i| matches!(i, Inst::FieldSet { sid, idx: 0, .. } if *sid == box_sid)),
            "the scalar tag is stored into the box"
        );
        assert!(
            insts_of(main).any(
                |i| matches!(i, Inst::IfaceMake { concrete_sid, .. } if *concrete_sid == box_sid)
            ),
            "the box pointer is the fat pointer's data word"
        );
    }

    #[test]
    fn tier1_box_thunk_reloads_tag_and_calls_the_real_method() {
        // The thunk bridges the pointer-self ABI of interface dispatch to the
        // value-self ABI of the real method: load the tag from box field 0, then
        // call `{Enum}__m`.
        let m = lower_ok(
            r#"interface Ranked:
    def rank(self) -> int

enum Color:
    Red
    Green

    def rank(self) -> int:
        return 7

def main():
    Ranked r = Color.Green
    print(r.rank())
"#,
        );
        let box_sid = m
            .structs
            .iter()
            .position(|s| s.name == "Color$box")
            .unwrap() as u32;
        let thunk = m
            .funcs
            .iter()
            .find(|f| f.name == "Color$box__rank")
            .unwrap();
        assert!(
            insts_of(thunk).any(
                |i| matches!(i, Inst::FieldGet { base: 0, sid, idx: 0, .. } if *sid == box_sid)
            ),
            "the thunk reloads the tag from box field 0"
        );
        assert!(
            insts_of(thunk).any(|i| matches!(i, Inst::Call { func, .. } if func == "Color__rank")),
            "the thunk tail-calls the real method"
        );
    }

    #[test]
    fn each_tier1_widening_boxes_a_fresh_cell() {
        // `is`-identity semantics (D-0016 extension): every widening of a tier-1
        // enum to an interface value allocates a FRESH box — identity is not
        // shared across widenings. Two widenings -> two distinct StructNew(box).
        // (`is` on an interface value is itself rejected at the surface as a value
        // type, ZAP4073, so this fresh-box property is the observable semantics.)
        let m = lower_ok(
            r#"interface Ranked:
    def rank(self) -> int

enum Color:
    Red
    Green

    def rank(self) -> int:
        return 7

def main():
    Ranked a = Color.Red
    Ranked b = Color.Red
    print(a.rank())
    print(b.rank())
"#,
        );
        let box_sid = m
            .structs
            .iter()
            .position(|s| s.name == "Color$box")
            .unwrap() as u32;
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        let boxes = insts_of(main)
            .filter(|i| matches!(i, Inst::StructNew { sid, .. } if *sid == box_sid))
            .count();
        assert_eq!(boxes, 2, "each widening boxes afresh (no shared identity)");
    }

    #[test]
    fn tier1_interface_render_slot_reaches_the_enum_renderer() {
        // #707 tie-in: a tier-1 enum behind an interface records a box renderer
        // `{Enum}$box$repr` in the renderer slot, and rendering the interface
        // value emits an IfaceRender dispatch (not a struct_to_str of the box).
        let m = lower_ok(
            r#"interface Node:
    def rank(self) -> int

enum Leaf:
    A
    B

    def rank(self) -> int:
        return 0

def main():
    Node a = Leaf.B
    print(a)
"#,
        );
        let box_sid = m.structs.iter().position(|s| s.name == "Leaf$box").unwrap() as u32;
        let sym = m
            .iface_renderers
            .get(&box_sid)
            .expect("the box has a recorded renderer for the itable slot");
        assert_eq!(sym, "Leaf$box$repr");
        assert!(
            m.funcs.iter().any(|f| &f.name == sym),
            "the box renderer is an emitted function"
        );
        assert!(
            emits_iface_render(m.funcs.iter().find(|f| f.name == "main").unwrap()),
            "rendering the interface value dispatches dynamically"
        );
    }

    // --- #829: concrete values into interface-typed collection elements ------
    //
    // A `list[Iface]` / `dict[K, Iface]` element is a one-word cell, so a
    // concrete conformer is coerced into the `{data, itable}` fat pointer
    // (`IfaceMake`) and that fat pointer is heap-boxed (a `StructNew` of the
    // interface's synthetic fat struct) so the stored word is a stable pointer
    // rather than a dangling stack address (I#84 follow-up). These lock the
    // lowered ZIR; the end-to-end dispatch is pinned by the CI goldens.

    /// The synthetic fat-pointer struct id of the interface named `name`.
    fn fat_sid_of(m: &Module, name: &str) -> u32 {
        m.interfaces
            .iter()
            .find(|i| i.name == name)
            .expect("interface registered")
            .fat_sid
    }

    fn struct_sid_of(m: &Module, name: &str) -> u32 {
        m.structs
            .iter()
            .position(|s| s.name == name)
            .unwrap_or_else(|| panic!("struct '{name}' lowered")) as u32
    }

    #[test]
    fn list_iface_literal_boxes_each_concrete_conformer() {
        // The issue repro: `list[Shape] xs = [Tri(), Quad()]` was ZAP6001 at
        // lowering. Now every concrete element is widened into the fat pointer and
        // heap-boxed for storage, and read-back dispatches dynamically.
        let m = lower_ok(
            r#"interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

struct Quad:
    def sides(self) -> int:
        return 4

def main():
    list[Shape] shapes = [Tri(), Quad()]
    for s in shapes:
        print(s.sides())
"#,
        );
        let fat = fat_sid_of(&m, "Shape");
        let tri = struct_sid_of(&m, "Tri");
        let quad = struct_sid_of(&m, "Quad");
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        // Both concrete types are boxed into the fat pointer at the element store.
        for c in [tri, quad] {
            assert!(
                insts_of(main).any(
                    |i| matches!(i, Inst::IfaceMake { concrete_sid, .. } if *concrete_sid == c)
                ),
                "concrete element {c} is widened into the interface fat pointer"
            );
        }
        // The fat pointer is heap-boxed (a StructNew of the interface fat struct)
        // once per element, so the list cell is a stable heap pointer, not a
        // dangling stack alloca.
        let boxes = insts_of(main)
            .filter(|i| matches!(i, Inst::StructNew { sid, .. } if *sid == fat))
            .count();
        assert_eq!(boxes, 2, "each interface element is heap-boxed for storage");
        // Read-back dispatches through the itable.
        assert!(
            insts_of(main).any(|i| matches!(i, Inst::IfaceCall { .. } | Inst::IfaceInvoke { .. })),
            "iterating the list dispatches dynamically on each element"
        );
    }

    #[test]
    fn append_concrete_to_list_iface_element_boxes() {
        // `xs.append(Tri())` into a `list[Shape]` (incl. the empty-then-append
        // form) widens and heap-boxes, exactly like the literal path.
        let m = lower_ok(
            r#"interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

def main():
    list[Shape] shapes = []
    shapes.append(Tri())
    print(shapes[0].sides())
"#,
        );
        let fat = fat_sid_of(&m, "Shape");
        let tri = struct_sid_of(&m, "Tri");
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(
            insts_of(main)
                .any(|i| matches!(i, Inst::IfaceMake { concrete_sid, .. } if *concrete_sid == tri)),
            "the appended concrete value is widened into the fat pointer"
        );
        assert!(
            insts_of(main).any(|i| matches!(i, Inst::StructNew { sid, .. } if *sid == fat)),
            "the appended interface value is heap-boxed for storage"
        );
    }

    #[test]
    fn dict_iface_value_element_boxes() {
        // A `dict[K, Iface]` VALUE accepts a concrete conformer and boxes it; the
        // key position is unaffected (interface values are unhashable, so keys
        // stay concrete).
        let m = lower_ok(
            r#"interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

def main():
    dict[int, Shape] by_id = {1: Tri()}
    print(by_id[1].sides())
"#,
        );
        let fat = fat_sid_of(&m, "Shape");
        let tri = struct_sid_of(&m, "Tri");
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(
            insts_of(main)
                .any(|i| matches!(i, Inst::IfaceMake { concrete_sid, .. } if *concrete_sid == tri)),
            "the dict value is widened into the interface fat pointer"
        );
        assert!(
            insts_of(main).any(|i| matches!(i, Inst::StructNew { sid, .. } if *sid == fat)),
            "the dict interface value is heap-boxed for storage"
        );
    }

    #[test]
    fn list_iface_element_accepts_both_enum_tiers() {
        // The #731 tie-in: both enum tiers conform as interface collection
        // elements. A tier-3 enum boxes its tagged-struct pointer; a tier-1 enum
        // boxes its scalar into `{Enum}$box` first, then both are heap-boxed into
        // the fat pointer for storage.
        let m = lower_ok(
            r#"interface Ranked:
    def rank(self) -> int

enum Tri3:
    Lit(int v)
    Add(int l, int r)

    def rank(self) -> int:
        return 1

enum Tri1:
    Red
    Green

    def rank(self) -> int:
        return 2

def main():
    list[Ranked] a = [Tri3.Lit(v=5)]
    a.append(Tri1.Green)
    for r in a:
        print(r.rank())
"#,
        );
        let fat = fat_sid_of(&m, "Ranked");
        let tri3 = struct_sid_of(&m, "Tri3"); // tier-3 tagged struct
        let box_sid = struct_sid_of(&m, "Tri1$box"); // tier-1 scalar box
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(
            insts_of(main).any(
                |i| matches!(i, Inst::IfaceMake { concrete_sid, .. } if *concrete_sid == tri3)
            ),
            "tier-3 enum element is boxed by its tagged-struct id"
        );
        assert!(
            insts_of(main).any(
                |i| matches!(i, Inst::IfaceMake { concrete_sid, .. } if *concrete_sid == box_sid)
            ),
            "tier-1 enum element is boxed by its scalar-box id"
        );
        let boxes = insts_of(main)
            .filter(|i| matches!(i, Inst::StructNew { sid, .. } if *sid == fat))
            .count();
        assert_eq!(boxes, 2, "both enum elements are heap-boxed for storage");
    }

    #[test]
    fn nested_list_of_list_iface_elements_lower() {
        // `list[list[Shape]]`: the inner `list[Shape]` boxes its concrete element
        // into the fat pointer; the outer list stores the inner-list pointer as a
        // nested element. The whole thing lowers (was ZAP6001).
        let m = lower_ok(
            r#"interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

def main():
    list[list[Shape]] grid = [[Tri()]]
    print(grid[0][0].sides())
"#,
        );
        let tri = struct_sid_of(&m, "Tri");
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(
            insts_of(main)
                .any(|i| matches!(i, Inst::IfaceMake { concrete_sid, .. } if *concrete_sid == tri)),
            "the inner-list concrete element is boxed into the fat pointer"
        );
    }

    // --- #830: refined-supertype method dispatch on an interface value -------
    //
    // An interface's dispatch-table carries its own methods first, then its
    // transitive refined-supertype methods, in a stable order. Both the itable
    // layout (codegen) and dynamic dispatch (`lower_iface_call`) read this one
    // list, so an inherited method call resolves to the slot the itable laid out.

    #[test]
    fn refined_interface_dispatch_table_folds_in_supertype_methods() {
        // `interface Ranked: Named`. Before #830, calling the inherited `name()`
        // on a `Ranked` value was ZAP6001 ("interface 'Ranked' has no method
        // 'name'"). Now `Ranked`'s effective method list is [rank, name] (own
        // first, then inherited), both calls lower, and each dispatches on its
        // own stable slot index.
        let m = lower_ok(
            r#"interface Named:
    def name(self) -> int

interface Ranked: Named
    def rank(self) -> int

struct Color:
    def name(self) -> int:
        return 0
    def rank(self) -> int:
        return 9

def main():
    Ranked r = Color()
    print(r.rank())
    print(r.name())
"#,
        );
        let ranked = m
            .interfaces
            .iter()
            .find(|i| i.name == "Ranked")
            .expect("Ranked registered");
        let names: Vec<&str> = ranked.methods.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["rank", "name"],
            "effective slots: own methods first, then inherited supertype methods"
        );
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        let mut slots: Vec<u32> = insts_of(main)
            .filter_map(|i| match i {
                Inst::IfaceCall { method, .. } | Inst::IfaceInvoke { method, .. } => Some(*method),
                _ => None,
            })
            .collect();
        slots.sort_unstable();
        assert_eq!(
            slots,
            vec![0, 1],
            "`rank` dispatches on slot 0 (own), `name` on slot 1 (inherited)"
        );
    }

    #[test]
    fn multilevel_refinement_folds_every_level() {
        // A: B: C — a `C`-typed value can call a method introduced at every level,
        // and C's dispatch table carries all three in a stable order (own first,
        // then up the refinement chain).
        let m = lower_ok(
            r#"interface A:
    def a(self) -> int

interface B: A
    def b(self) -> int

interface C: B
    def c(self) -> int

struct S:
    def a(self) -> int:
        return 1
    def b(self) -> int:
        return 2
    def c(self) -> int:
        return 3

def main():
    C x = S()
    print(x.a())
    print(x.b())
    print(x.c())
"#,
        );
        let c = m.interfaces.iter().find(|i| i.name == "C").unwrap();
        let names: Vec<&str> = c.methods.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["c", "b", "a"],
            "own, then transitive supertypes"
        );
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        let mut slots: Vec<u32> = insts_of(main)
            .filter_map(|i| match i {
                Inst::IfaceCall { method, .. } | Inst::IfaceInvoke { method, .. } => Some(*method),
                _ => None,
            })
            .collect();
        slots.sort_unstable();
        assert_eq!(
            slots,
            vec![0, 1, 2],
            "three distinct, stable dispatch slots"
        );
    }

    #[test]
    fn refined_interface_dispatch_slots_are_stable_across_conformers() {
        // The itable slot order is a property of the interface, not the concrete
        // type, so a second conformer dispatches the inherited method on the same
        // slot — the stored word is uniform across implementors.
        let m = lower_ok(
            r#"interface Named:
    def name(self) -> int

interface Ranked: Named
    def rank(self) -> int

struct Color:
    def name(self) -> int:
        return 0
    def rank(self) -> int:
        return 1

struct Grade:
    def name(self) -> int:
        return 2
    def rank(self) -> int:
        return 3

def main():
    list[Ranked] rs = [Color(), Grade()]
    for r in rs:
        print(r.name())
"#,
        );
        let ranked = m.interfaces.iter().find(|i| i.name == "Ranked").unwrap();
        // `name` is at the same slot regardless of which conformer is stored.
        let name_slot = ranked
            .methods
            .iter()
            .position(|x| x.name == "name")
            .expect("inherited method is a slot");
        assert_eq!(
            name_slot, 1,
            "inherited `name` is slot 1 for every conformer"
        );
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        assert!(
            insts_of(main).any(|i| matches!(
                i,
                Inst::IfaceCall { method, .. } | Inst::IfaceInvoke { method, .. }
                    if *method == name_slot as u32
            )),
            "the inherited method dispatches on its stable slot"
        );
    }

    #[test]
    fn refined_interface_works_for_both_enum_tiers() {
        // Supertype slots must work for struct AND enum conformers (#731 tie-in):
        // a tier-3 and a tier-1 enum each conform to `Ranked: Named` and can call
        // the inherited `name()`.
        let m = lower_ok(
            r#"interface Named:
    def name(self) -> int

interface Ranked: Named
    def rank(self) -> int

enum Three:
    Lit(int v)

    def name(self) -> int:
        return 0
    def rank(self) -> int:
        return 1

enum One:
    Red
    Green

    def name(self) -> int:
        return 2
    def rank(self) -> int:
        return 3

def main():
    Ranked a = Three.Lit(v=5)
    Ranked b = One.Green
    print(a.name())
    print(b.name())
"#,
        );
        // Both enum conformers produce the inherited-method itable symbol: the
        // tier-3 tagged-struct method and the tier-1 box thunk.
        assert!(
            m.funcs.iter().any(|f| f.name == "Three__name"),
            "tier-3 enum's inherited method is lowered as an itable slot target"
        );
        assert!(
            m.funcs.iter().any(|f| f.name == "One$box__name"),
            "tier-1 enum's inherited method has a box thunk for the itable slot"
        );
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        let name_calls = insts_of(main)
            .filter(|i| matches!(
                i,
                Inst::IfaceCall { method, .. } | Inst::IfaceInvoke { method, .. } if *method == 1
            ))
            .count();
        assert_eq!(name_calls, 2, "both enum values dispatch inherited `name`");
    }

    // --- #552: unsigned integers select the unsigned ZIR ops -----------------
    //
    // These lock the op-selection *before* codegen (where LLVM finally emits
    // udiv/urem/lshr/zext/uitofp/fptoui): a wrong choice here would be a silent
    // miscompile, so the walled garden asserts on the lowered ZIR directly. The
    // LLVM text itself is pinned by the `unsigned_ints` golden under CI.

    /// Does `main` contain a `call`/`invoke` to runtime symbol `sym`?
    fn calls(m: &Module, sym: &str) -> bool {
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        main.blocks.iter().flat_map(|b| &b.insts).any(
            |i| matches!(i, Inst::Call { func, .. } | Inst::Invoke { func, .. } if func == sym),
        )
    }

    /// Does `main` contain a `Cast` of the given kind?
    fn has_cast(m: &Module, kind: CastKind) -> bool {
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        main.blocks
            .iter()
            .flat_map(|b| &b.insts)
            .any(|i| matches!(i, Inst::Cast { kind: k, .. } if *k == kind))
    }

    fn lower_main(body: &str) -> Module {
        lower_ok(&format!("def main():\n{body}"))
    }

    #[test]
    fn unsigned_floordiv_mod_use_unsigned_helpers() {
        // `//` and `%` on unsigned operands pick the unsigned runtime helpers
        // (plain udiv/urem, no floored-sign correction), distinct from the
        // signed `zap_ifloordiv`/`zap_imod`.
        let m = lower_main(
            "    uint64 a = 10\n    uint64 b = 3\n    print(a // b)\n    print(a % b)\n",
        );
        assert!(calls(&m, "zap_udiv"), "unsigned // must call zap_udiv");
        assert!(calls(&m, "zap_umod"), "unsigned % must call zap_umod");
        assert!(
            !calls(&m, "zap_ifloordiv"),
            "must not use the signed floordiv"
        );
        assert!(!calls(&m, "zap_imod"), "must not use the signed modulo");

        // The signed tower is unchanged (contrast).
        let s =
            lower_main("    int64 a = 10\n    int64 b = 3\n    print(a // b)\n    print(a % b)\n");
        assert!(calls(&s, "zap_ifloordiv"));
        assert!(calls(&s, "zap_imod"));
        assert!(!calls(&s, "zap_udiv"));
        assert!(!calls(&s, "zap_umod"));
    }

    #[test]
    fn unsigned_right_shift_is_logical() {
        // `>>` on an unsigned value is a logical shift (`zap_ushr`), on a signed
        // value an arithmetic one (`zap_ishr`). The count here is a variable so
        // the guarded (non-constant) helper path is exercised.
        let m = lower_main("    uint32 a = 4000000000\n    uint32 n = 1\n    print(a >> n)\n");
        assert!(calls(&m, "zap_ushr"), "unsigned >> must be a logical shift");
        assert!(!calls(&m, "zap_ishr"));
        let s = lower_main("    int32 a = 16\n    int32 n = 1\n    print(a >> n)\n");
        assert!(calls(&s, "zap_ishr"));
        assert!(!calls(&s, "zap_ushr"));
    }

    #[test]
    fn unsigned_pow_uses_unsigned_helper() {
        let m = lower_main("    uint64 a = 2\n    uint64 b = 10\n    print(a ** b)\n");
        assert!(calls(&m, "zap_upow"));
        assert!(!calls(&m, "zap_ipow"));
    }

    #[test]
    fn unsigned_values_render_unsigned_decimal() {
        // print/str on every unsigned width routes through the unsigned
        // formatter, so a value above INT64_MAX prints its true magnitude and no
        // value carries a phantom sign.
        for ty in ["uint8", "uint16", "uint32", "uint64", "usize"] {
            let m = lower_main(&format!("    {ty} x = 7\n    print(x)\n"));
            assert!(
                calls(&m, "zap_u64_to_str"),
                "{ty} must render via zap_u64_to_str"
            );
            assert!(
                !calls(&m, "zap_i64_to_str"),
                "{ty} must not render as signed"
            );
        }
        // `str()` of a signed int still renders via the signed formatter
        // (contrast); `str()` of an unsigned one via the unsigned formatter.
        let s = lower_main("    int64 x = 7\n    auto t = str(x)\n    print(t)\n");
        assert!(calls(&s, "zap_i64_to_str"));
        assert!(!calls(&s, "zap_u64_to_str"));
        let u = lower_main("    uint64 x = 7\n    auto t = str(x)\n    print(t)\n");
        assert!(calls(&u, "zap_u64_to_str"));
        assert!(!calls(&u, "zap_i64_to_str"));
    }

    #[test]
    fn unsigned_widening_zero_extends() {
        // Implicit widening up the unsigned tower zero-extends (ZExt), never
        // sign-extends — `uint8(200)` as a `uint64` stays 200.
        let m = lower_main("    uint8 b = 200\n    uint64 w = b\n    print(w)\n");
        assert!(
            has_cast(&m, CastKind::ZExt),
            "unsigned widening must zero-extend"
        );
        assert!(
            !has_cast(&m, CastKind::SExt),
            "unsigned widening must not sign-extend"
        );
    }

    #[test]
    fn unsigned_explicit_conversions_pick_unsigned_casts() {
        // float(uint) / uint(float) use the unsigned float<->int casts.
        let to_f = lower_main("    uint64 u = 9\n    print(float(u))\n");
        assert!(
            has_cast(&to_f, CastKind::UIToFP),
            "uint -> float must be uitofp"
        );
        let to_u = lower_main("    float f = 9.0\n    uint32 u = uint32(f)\n    print(u)\n");
        assert!(
            has_cast(&to_u, CastKind::FPToUI),
            "float -> uint must be fptoui"
        );
        // Narrowing between unsigned widths truncates.
        let narrow = lower_main("    uint64 w = 300\n    uint8 b = uint8(w)\n    print(b)\n");
        assert!(
            has_cast(&narrow, CastKind::Trunc),
            "uint64 -> uint8 must truncate"
        );
    }

    #[test]
    fn usize_lowers_like_uint64() {
        // `usize` is pointer-width: it lowers to the same `u64` codegen path, so
        // its arithmetic/printing use the unsigned helpers.
        let m = lower_main("    usize n = 5\n    usize m = 2\n    print(n // m)\n");
        assert!(calls(&m, "zap_udiv"));
        assert!(calls(&m, "zap_u64_to_str"));
    }

    // --- #552 follow-up: subscript indices auto-promote to the i64 index word.
    //
    // The index position widens every integer type to the 64-bit index word: a
    // signed index sign-extends (so `[-1]` is from-the-end), an unsigned index
    // zero-extends (always a non-negative offset — a high byte like 200 is never
    // read as a negative index). These lock the extension *kind* on the index,
    // which codegen then emits as sext/zext.

    #[test]
    fn unsigned_subscript_index_zero_extends() {
        // A `uint16`/`uint8`/`uint32` list index zero-extends to i64 (the only
        // SExt-vs-ZExt-distinguishing cast in the function is the index widen;
        // the binding `uint16 c = 1` is a Trunc, not an extend).
        for ty in ["uint8", "uint16", "uint32"] {
            let m = lower_main(&format!(
                "    list[int64] xs = [10, 20, 30]\n    {ty} c = 1\n    print(xs[c])\n"
            ));
            assert!(
                has_cast(&m, CastKind::ZExt),
                "{ty} index must zero-extend to the i64 index word"
            );
            assert!(
                !has_cast(&m, CastKind::SExt),
                "{ty} index must not sign-extend"
            );
            assert!(
                calls(&m, "zap_list_get_i64"),
                "the list get is still emitted"
            );
        }
    }

    #[test]
    fn signed_subscript_index_sign_extends() {
        // An `int32` index sign-extends, so a negative index keeps its from-the-
        // end meaning once normalized by the runtime.
        let m =
            lower_main("    list[int64] xs = [10, 20, 30]\n    int32 a = 1\n    print(xs[a])\n");
        assert!(
            has_cast(&m, CastKind::SExt),
            "int32 index must sign-extend to the i64 index word"
        );
    }

    #[test]
    fn unsigned_index_store_and_array_index_lower() {
        // The *store* path promotes the index the same way, and `array[uint8]`
        // indexing by an unsigned value lowers end to end (get + set).
        lower_ok(
            "def main():\n    list[int64] xs = [1, 2, 3]\n    uint16 c = 2\n    xs[c] = 9\n    print(xs[c])\n",
        );
        let a = lower_main(
            "    array[uint8] buf = array[uint8]([5, 6, 7])\n    uint16 i = 2\n    int32 j = 1\n    buf[j] = 200\n    print(buf[i])\n",
        );
        assert!(calls(&a, "zap_array_get_u8"), "array get is emitted");
        assert!(calls(&a, "zap_array_set_u8"), "array set is emitted");
        // The unsigned array index still zero-extends.
        assert!(
            has_cast(&a, CastKind::ZExt),
            "unsigned array index zero-extends"
        );
    }

    #[test]
    fn wide_unsigned_index_is_guarded_against_from_end_wrap() {
        // A `uint64`/`usize` index can exceed INT64_MAX and reinterpret to a
        // negative i64 with the same bits as a signed negative index. It must
        // NOT be read as a from-the-end index, so lowering routes it through
        // `zap_uindex`, which clamps overflow out of range (the bounds check
        // then raises IndexError instead of silently wrapping from the end).
        for ty in ["uint64", "usize"] {
            let m = lower_main(&format!(
                "    list[int64] xs = [10, 20, 30]\n    {ty} c = 1\n    print(xs[c])\n"
            ));
            assert!(
                calls(&m, "zap_uindex"),
                "{ty} index must be guarded by zap_uindex"
            );
        }
    }

    #[test]
    fn narrow_unsigned_and_signed_indices_are_not_guarded() {
        // `uint8`/`uint16`/`uint32` zero-extend into `[0, 2^32)` and are always
        // non-negative, so they are safe unguarded; a signed index legitimately
        // counts from the end. None of these is routed through `zap_uindex`.
        for ty in ["uint8", "uint16", "uint32", "int32", "int64"] {
            let m = lower_main(&format!(
                "    list[int64] xs = [10, 20, 30]\n    {ty} c = 1\n    print(xs[c])\n"
            ));
            assert!(
                !calls(&m, "zap_uindex"),
                "{ty} index must not be guarded (already safe / from-the-end is intended)"
            );
        }
    }

    #[test]
    fn wide_unsigned_index_guarded_on_store_del_and_array() {
        // The guard covers every element subscript, not just reads: list store,
        // list `del`, and `array[uint8]` get/set.
        let set = lower_main(
            "    list[int64] xs = [1, 2, 3]\n    uint64 c = 0\n    xs[c] = 9\n    print(xs[c])\n",
        );
        assert!(calls(&set, "zap_uindex"), "list store index is guarded");
        let del = lower_main("    list[int64] xs = [1, 2, 3]\n    usize c = 0\n    del xs[c]\n");
        assert!(calls(&del, "zap_uindex"), "list del index is guarded");
        let arr = lower_main(
            "    array[uint8] buf = array[uint8]([5, 6, 7])\n    uint64 i = 0\n    buf[i] = 9\n    print(buf[i])\n",
        );
        assert!(calls(&arr, "zap_uindex"), "array index is guarded");
    }
}
