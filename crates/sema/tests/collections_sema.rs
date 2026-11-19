//! `collections` module — sema (type-check) behaviour (#972, epic #966).
//!
//! Host-only (parse + sema, no LLVM). Locks the type surface of the Python
//! `collections` conveniences, each a generic prelude struct over the built-in
//! `dict`/`list` reached through the module-qualified seam sugar (D-0202):
//!
//!   * `collections.Counter[K]`        (backing prelude struct `CounterMap`)
//!   * `collections.defaultdict[K, V]` (backing prelude struct `DefaultDict`)
//!   * `collections.OrderedDict[K, V]` (backing prelude struct `OrderedMap`)
//!
//! The backing structs are renamed so the Python name `Counter` does not collide
//! with the already-shipped lock-free `concurrent.Counter` (D-0078); a user never
//! spells the backing name. End-to-end execution + semantics (grouping, counts,
//! ordering) are exercised by the golden `tests/golden/collections_module.zap`.

use zapc_sema::check_str;

#[track_caller]
fn errors(src: &str) -> Vec<(String, String)> {
    let (terrs, perrs, lerrs) = check_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    terrs
        .into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect()
}

#[track_caller]
fn ok(src: &str) {
    let e = errors(src);
    assert!(e.is_empty(), "expected no sema errors, got: {e:?}");
}

#[track_caller]
fn code(src: &str, want: &str) {
    let e = errors(src);
    assert!(
        e.iter().any(|(c, _)| c == want),
        "expected a {want} error, got: {e:?}",
    );
}

const IMP: &str = "import collections\n";

// ---------------------------------------------------------------------------
// import recognition
// ---------------------------------------------------------------------------

#[test]
fn import_collections_is_recognized() {
    // A bare `import collections` (no use yet) must resolve to the built-in
    // module, not be treated as an unknown user module.
    ok("import collections\ndef f():\n    pass\n");
}

// ---------------------------------------------------------------------------
// Counter — construction
// ---------------------------------------------------------------------------

#[test]
fn counter_from_iterable_and_empty() {
    ok(&format!(
        "{IMP}def f():\n    collections.Counter[str] c = collections.Counter([\"a\", \"b\", \"a\"])\n    print(c.total())\n"
    ));
    // Empty form: element type from the annotation.
    ok(&format!(
        "{IMP}def f():\n    collections.Counter[str] c = collections.Counter()\n    print(len(c))\n"
    ));
    // Explicit type argument on the dotted constructor.
    ok(&format!(
        "{IMP}def f():\n    auto c = collections.Counter[int64]([1, 1, 2])\n    print(c.total())\n"
    ));
}

#[test]
fn counter_keyword_construction() {
    ok(&format!(
        "{IMP}def f():\n    auto c = collections.Counter[str](items=[\"a\", \"a\"])\n    print(c.total())\n"
    ));
}

#[test]
fn counter_int_keys() {
    ok(&format!(
        "{IMP}def f():\n    collections.Counter[int64] c = collections.Counter([10, 10, 20])\n    print(c[10])\n"
    ));
}

#[test]
fn counter_as_param_and_return_type() {
    ok(&format!(
        "{IMP}def tally(list[str] xs) -> collections.Counter[str]:\n    return collections.Counter(xs)\ndef f():\n    auto c = tally([\"a\"])\n    print(c.total())\n"
    ));
}

// ---------------------------------------------------------------------------
// Counter — surface
// ---------------------------------------------------------------------------

#[test]
fn counter_index_and_membership_and_len() {
    ok(&format!(
        "{IMP}def f():\n    collections.Counter[str] c = collections.Counter()\n    int64 n = c[\"a\"]\n    bool has = \"a\" in c\n    int64 k = len(c)\n    print(n, has, k)\n"
    ));
}

#[test]
fn counter_mutators_and_accessors() {
    ok(&format!(
        "{IMP}def f():\n    collections.Counter[str] c = collections.Counter()\n    c.add(\"a\")\n    c.add(\"b\", 3)\n    c.subtract(\"a\")\n    c.subtract(\"b\", 2)\n    c.update([\"a\", \"c\"])\n    c[\"d\"] = 9\n    int64 t = c.total()\n    int64 g = c.get(\"a\")\n    list[str] ks = c.keys()\n    print(t, g, len(ks))\n"
    ));
}

#[test]
fn counter_add_subtract_keyword_arg() {
    ok(&format!(
        "{IMP}def f():\n    collections.Counter[str] c = collections.Counter()\n    c.add(key=\"a\", n=2)\n    c.subtract(key=\"a\", n=1)\n    print(c[\"a\"])\n"
    ));
}

#[test]
fn counter_most_common_shape() {
    // most_common(n) and most_common() both yield list[(K, int64)].
    ok(&format!(
        "{IMP}def f():\n    collections.Counter[str] c = collections.Counter([\"a\", \"a\", \"b\"])\n    list[(str, int64)] top = c.most_common(1)\n    list[(str, int64)] all_ = c.most_common()\n    print(top[0][1], all_[0][1])\n"
    ));
}

#[test]
fn counter_most_common_keyword_arg() {
    ok(&format!(
        "{IMP}def f():\n    collections.Counter[str] c = collections.Counter([\"a\"])\n    auto top = c.most_common(n=1)\n    print(len(top))\n"
    ));
}

#[test]
fn counter_elements_yields_key_list() {
    ok(&format!(
        "{IMP}def f():\n    collections.Counter[str] c = collections.Counter([\"a\", \"a\"])\n    list[str] els = c.elements()\n    print(len(els))\n"
    ));
}

#[test]
fn counter_add_sub_operators() {
    ok(&format!(
        "{IMP}def f():\n    collections.Counter[str] a = collections.Counter([\"x\", \"x\"])\n    collections.Counter[str] b = collections.Counter([\"x\", \"y\"])\n    auto s = a + b\n    auto d = a - b\n    print(s[\"x\"], d[\"x\"])\n"
    ));
}

// ---------------------------------------------------------------------------
// defaultdict
// ---------------------------------------------------------------------------

#[test]
fn defaultdict_list_factory_grouping() {
    ok(&format!(
        "{IMP}def mk() -> list[int64]:\n    list[int64] e = []\n    return e\ndef f():\n    collections.defaultdict[str, list[int64]] d = collections.defaultdict(mk)\n    d[\"g\"].append(1)\n    d[\"g\"].append(2)\n    print(len(d[\"g\"]))\n"
    ));
}

#[test]
fn defaultdict_int_factory_accumulate() {
    ok(&format!(
        "{IMP}def zero() -> int64:\n    return 0\ndef f():\n    collections.defaultdict[str, int64] c = collections.defaultdict(zero)\n    c[\"a\"] = c[\"a\"] + 1\n    print(c[\"a\"])\n"
    ));
}

#[test]
fn defaultdict_keyword_factory_and_surface() {
    ok(&format!(
        "{IMP}def zero() -> int64:\n    return 0\ndef f():\n    auto d = collections.defaultdict[str, int64](factory=zero)\n    d[\"a\"] = 5\n    bool has = \"a\" in d\n    int64 n = len(d)\n    int64 g = d.get(\"b\", 0)\n    list[str] ks = d.keys()\n    print(has, n, g, len(ks))\n"
    ));
}

#[test]
fn defaultdict_custom_struct_factory() {
    ok(&format!(
        "{IMP}struct Bag:\n    int64 size\ndef mk() -> Bag:\n    return Bag(size=0)\ndef f():\n    collections.defaultdict[str, Bag] d = collections.defaultdict(mk)\n    auto b = d[\"k\"]\n    print(b.size)\n"
    ));
}

// ---------------------------------------------------------------------------
// OrderedDict
// ---------------------------------------------------------------------------

#[test]
fn ordered_dict_surface() {
    ok(&format!(
        "{IMP}def f():\n    collections.OrderedDict[str, int64] od = collections.OrderedDict()\n    od[\"a\"] = 1\n    od[\"b\"] = 2\n    int64 v = od[\"a\"]\n    bool has = \"a\" in od\n    int64 n = len(od)\n    int64 g = od.get(\"z\", 0)\n    list[str] ks = od.keys()\n    list[int64] vs = od.values()\n    list[(str, int64)] its = od.items()\n    od.move_to_end(\"a\")\n    od.delete(\"b\")\n    print(v, has, n, g, len(ks), len(vs), len(its))\n"
    ));
}

#[test]
fn ordered_dict_explicit_type_args() {
    ok(&format!(
        "{IMP}def f():\n    auto od = collections.OrderedDict[int64, str]()\n    od[1] = \"one\"\n    print(od[1])\n"
    ));
}

// ---------------------------------------------------------------------------
// name-clash isolation: collections.Counter vs concurrent.Counter
// ---------------------------------------------------------------------------

#[test]
fn collections_counter_distinct_from_concurrent_counter() {
    // Both modules expose a `Counter`; they are different types and coexist.
    // `concurrent.Counter` is the lock-free integer counter (has `.incr()`),
    // `collections.Counter` is the multiset (has `.total()`); each only answers
    // its own surface. (That `concurrent.Counter` has NO multiset methods is
    // pinned at compile time by the `collections_concurrent_counter_distinct`
    // negative golden, since the method-not-found diagnostic is a lowering-stage
    // check beyond this sema-only harness.)
    ok("import collections\nimport concurrent\ndef f():\n    auto lock = concurrent.Counter()\n    print(lock.incr())\n    collections.Counter[str] multi = collections.Counter([\"a\"])\n    print(multi.total())\n");
}

// ---------------------------------------------------------------------------
// negative — argument types and unknown surface (sema-stage)
//
// Cross-stage negatives — an unknown keyword on a constructor (a mono-stage
// `ZAP6001` on the mangled `new_counter$str`) and `concurrent.Counter` lacking
// the multiset surface (a lowering-stage `ZAP6001`) — are pinned by the negative
// goldens `collections_counter_bad_keyword` / `collections_concurrent_counter_distinct`,
// which run the full compile pipeline. This sema-only harness asserts the
// negatives the type checker itself raises.
// ---------------------------------------------------------------------------

#[test]
fn counter_rejects_non_iterable_argument() {
    code(
        &format!("{IMP}def f():\n    collections.Counter[str] c = collections.Counter(123)\n"),
        "ZAP4021",
    );
}

#[test]
fn collections_unknown_function_is_an_error() {
    // A `collections.foo(...)` that is not one of the seam-sugared constructors
    // is an unknown module function.
    code(
        &format!("{IMP}def f():\n    auto x = collections.frobnicate()\n"),
        "ZAP3003",
    );
}
