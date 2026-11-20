//! Lowering coverage for `float64` as a collection element/key (#800, #803).
//! These run on the host (no LLVM): they exercise AST→ZIR lowering directly,
//! independent of the sema gate, so the element-storage paths are a walled
//! garden. Phase 1 (dict VALUES, channel, heap) and Phase 2 (dict KEYS, set
//! MEMBERS) must all lower cleanly; only an `f32` element must still raise a
//! lowering error.
use crate::lower::lower_module;
use zapc_ir_core::Inst;

fn lower_errs(src: &str) -> Vec<String> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty(), "parse errors in test source: {perrs:?}");
    assert!(lerrs.is_empty(), "lex errors in test source: {lerrs:?}");
    let (_m, errs) = lower_module(&module);
    errs.into_iter().map(|e| format!("{e:?}")).collect()
}

/// The names of every `Inst::Invoke` (an unwinding call to a landing pad)
/// emitted anywhere in a lowered program — used to prove a float key/member
/// operation inside a `try` is catchable.
fn invoke_funcs(src: &str) -> Vec<String> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty(), "parse errors in test source: {perrs:?}");
    assert!(lerrs.is_empty(), "lex errors in test source: {lerrs:?}");
    let (m, errs) = lower_module(&module);
    assert!(errs.is_empty(), "unexpected lowering errors: {errs:?}");
    m.funcs
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .filter_map(|i| match i {
            Inst::Invoke { func, .. } => Some(func.clone()),
            _ => None,
        })
        .collect()
}

fn lowers_ok(src: &str) {
    let errs = lower_errs(src);
    assert!(
        errs.is_empty(),
        "unexpected lowering errors for\n{src}\n{errs:?}"
    );
}

fn lower_fails(src: &str) {
    let errs = lower_errs(src);
    assert!(!errs.is_empty(), "expected a lowering error for\n{src}");
}

#[test]
fn float_dict_values_lower() {
    // dict[K, float64] value — every key kind, construct/set/get/values.
    lowers_ok(
        "def main():\n    dict[str, float64] d = {}\n    d[\"a\"] = 1.5\n    \
             float x = d[\"a\"]\n    print(x)\n",
    );
    lowers_ok(
        "def main():\n    dict[int64, float64] d = {1: 1.5, 2: 2.5}\n    \
             float x = d[1]\n    print(x)\n",
    );
    lowers_ok("def main():\n    auto d = {\"a\": 1.5, \"b\": 2.5}\n    print(len(d))\n");
    lowers_ok(
        "def main():\n    dict[str, float64] d = {\"a\": 1.5}\n    \
             list[float64] vs = d.values()\n    print(len(vs))\n",
    );
}

#[test]
fn float_channel_elements_lower() {
    // channel[float64] — annotation + construction, send, receive.
    lowers_ok(
        "def main():\n    channel[float64] c = channel[float64](1)\n    \
             c <- 3.5\n    float x = <-c\n    print(x)\n",
    );
}

#[test]
fn float_heap_elements_lower() {
    // heap[float64] — min and max direction, push/pop/peek.
    lowers_ok(
        "def main():\n    heap[float64] h = heap[float64]()\n    h.push(3.5)\n    \
             h.push(1.5)\n    float x = h.pop()\n    print(x)\n",
    );
    lowers_ok(
        "def main():\n    auto h = heap[float64](cmp=\"max\")\n    h.push(1.5)\n    \
             float x = h.peek()\n    print(x)\n",
    );
}

#[test]
fn float_keys_and_members_lower() {
    // #800 Phase 2 (#803): a float dict KEY and a float set MEMBER lower to
    // the `_f64` key entry points (canonicalise `-0.0`, reject NaN — D-0138).
    // Construct, insert, read-back, membership, `.keys()`, `del`, and an
    // f64-keyed f64-valued dict all lower cleanly.
    lowers_ok(
        "def main():\n    dict[float64, str] d = {}\n    d[1.5] = \"a\"\n    \
             str x = d[1.5]\n    print(x)\n",
    );
    lowers_ok(
        "def main():\n    dict[float64, int64] d = {1.5: 1, 2.5: 2}\n    \
             print(1.5 in d)\n    del d[1.5]\n",
    );
    lowers_ok(
        "def main():\n    dict[float64, float64] d = {1.5: 2.5}\n    \
             float x = d[1.5]\n    print(x)\n",
    );
    lowers_ok(
        "def main():\n    dict[float64, str] d = {1.5: \"a\"}\n    \
             list[float64] ks = d.keys()\n    print(len(ks))\n",
    );
    lowers_ok(
        "def main():\n    set[float64] s = set[float64]()\n    s.add(1.5)\n    \
             print(1.5 in s)\n",
    );
    lowers_ok("def main():\n    auto s = {1.5, 2.5}\n    print(len(s))\n");
    lowers_ok("def main():\n    auto d = {1.5: 1}\n    print(len(d))\n");
}

#[test]
fn float_key_ops_in_try_lower_to_invoke() {
    // A float key/member op raises a *catchable* ValueError on a NaN key
    // (D-0138), so inside a `try` it MUST lower to an `Inst::Invoke` wired to a
    // landing pad — a plain `call` would unwind straight past the handler and
    // `terminate`. This is the regression guard for that wiring (the host
    // analogue of the `dict_float_key`/`set_float_member` goldens): each op,
    // wrapped in a `try`, appears among the program's invokes.
    let cases = [
        // insert, lookup, membership, delete on a float-keyed dict.
        (
            "d[1.5] = 5",
            "zap_dict_set_f64_i64",
            "dict[float64, int64] d = {}",
        ),
        (
            "int64 x = d[1.5]",
            "zap_dict_get_f64_i64",
            "dict[float64, int64] d = {1.5: 1}",
        ),
        (
            "bool b = 1.5 in d",
            "zap_dict_contains_f64",
            "dict[float64, int64] d = {}",
        ),
        (
            "del d[1.5]",
            "zap_dict_del_f64",
            "dict[float64, int64] d = {1.5: 1}",
        ),
        // add and membership on a float set.
        (
            "s.add(1.5)",
            "zap_set_add_f64",
            "set[float64] s = set[float64]()",
        ),
        (
            "bool b = 1.5 in s",
            "zap_set_contains_f64",
            "set[float64] s = set[float64]()",
        ),
    ];
    for (op, want, setup) in cases {
        let src = format!(
                "def main():\n    {setup}\n    try:\n        {op}\n    except ValueError:\n        print(\"caught\")\n"
            );
        let invokes = invoke_funcs(&src);
        assert!(
                invokes.iter().any(|f| f == want),
                "expected `{want}` to lower to an Invoke inside the try for:\n{src}\ngot invokes: {invokes:?}"
            );
    }
}

#[test]
fn f32_collection_elements_still_fail_to_lower() {
    // `f32` collection elements stay deferred (ticket #49) in every container.
    lower_fails("def main():\n    list[f32] xs = []\n");
    lower_fails("def main():\n    dict[str, f32] d = {}\n");
    lower_fails("def main():\n    heap[f32] h = heap[f32]()\n");
    lower_fails("def main():\n    channel[f32] c = channel[f32]()\n");
}
