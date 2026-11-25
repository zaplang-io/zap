//! Pins representative error inputs to their stable `ZAPnnnn` codes (I#157/F1,
//! issue #239), exercised through the MCP structured-check surface
//! (`check_source`) that coding agents consume.
//!
//! These tests guard the *codes*, not the message text: the whole point of the
//! scheme is that a code is stable while prose may change. A renumbering or an
//! accidental code change at an emission site breaks exactly here, which is the
//! intended regression signal. (Message text is deliberately not asserted.)

use zapc::mcp::check::{check_source, result_to_json};

/// Assert that checking `src` surfaces a diagnostic with `code` among its
/// diagnostics. We assert *presence*, not exclusivity, because recovery often
/// emits more than one diagnostic for one mistake.
#[track_caller]
fn assert_code(src: &str, code: &str) {
    let r = check_source(src);
    let got: Vec<String> = r.diagnostics.iter().map(|d| d.code.to_string()).collect();
    assert!(
        got.iter().any(|c| c == code),
        "expected {code} for source {src:?}, got {got:?}"
    );
}

#[test]
fn lexical_codes() {
    // Unterminated string literal.
    assert_code("def main():\n    x = \"oops\n", "ZAP1001");
    // Tab in indentation.
    assert_code("def main():\n\ty = 1\n", "ZAP1008");
}

#[test]
fn syntax_codes() {
    // A dangling binary operator — an expression was required.
    assert_code("def main():\n    x = 1 +\n", "ZAP2014");
    // A malformed `when target.*` predicate (I#139, D-0079).
    assert_code(
        "when target.cpu == .x86_64:\n    def f():\n        pass\n",
        "ZAP2015",
    );
    // An unknown architecture in a `when target.arch` predicate.
    assert_code(
        "when target.arch == .sparc:\n    def f():\n        pass\n",
        "ZAP2016",
    );
    // A malformed `as` pattern capture (#603, D11): `as` must be followed by a
    // single plain binding name, never `_`.
    assert_code(
        "def main():\n    match 1:\n        case 1 as _:\n            pass\n",
        "ZAP2021",
    );
}

#[test]
fn name_codes() {
    // Reference to an undefined name.
    assert_code("def main():\n    x = y\n", "ZAP3001");
    // Call to an undefined function.
    assert_code("def main():\n    nope()\n", "ZAP3003");
}

#[test]
fn type_codes() {
    // `int + str` — arithmetic on non-numbers.
    assert_code("def main():\n    x = 1 + \"a\"\n", "ZAP4012");
    // Division on non-numbers.
    assert_code("def main():\n    x = 1 / \"a\"\n", "ZAP4013");
    // Bitwise on non-integers.
    assert_code("def main():\n    x = 1 & \"a\"\n", "ZAP4014");
    // Returning the wrong type.
    assert_code(
        "def f() -> int32:\n    return \"x\"\n\ndef main():\n    pass\n",
        "ZAP4003",
    );
    // Calling a module `const` whose type does not define `__call__` (#1228):
    // the callable sugar requires the `Call` operator interface, so a
    // non-conformer is a clean "operator '()'" error, not an "unknown function".
    assert_code(
        "struct Plain:\n    int64 x\n\
         const Plain P = Plain(1)\n\
         def main():\n    auto r = P(5)\n",
        "ZAP4071",
    );
    // Associated functions (#1260). A receiverless `def` is called on the type;
    // calling it on an instance, or calling an instance method on the type, or
    // naming an unknown associated function, each has a dedicated code.
    const MSG: &str = concat!(
        "struct Message:\n",
        "    int64 id\n",
        "    def query(int64 id) -> Message:\n",
        "        return Message(id)\n",
        "    def to_wire(self) -> int64:\n",
        "        return self.id\n",
    );
    // Associated function called on an instance.
    assert_code(
        &format!("{MSG}def main():\n    auto m = Message.query(1)\n    auto b = m.query(2)\n"),
        "ZAP4085",
    );
    // Instance method called without a receiver.
    assert_code(
        &format!("{MSG}def main():\n    auto b = Message.to_wire()\n"),
        "ZAP4086",
    );
    // Unknown associated function on a known type (not "unknown name").
    assert_code(
        &format!("{MSG}def main():\n    auto b = Message.nope()\n"),
        "ZAP4087",
    );
}

#[test]
fn semantic_codes() {
    // `@inject` stays reclassified/declined for core — dependency injection is
    // the `Depends(...)` adapter, not a core decorator — rejected with ZAP5055
    // (I#472, D-0109). (`@cache` is no longer here: it ships in I#666/D-0115,
    // expanded before sema; its ZAP5056 signature checks run in the driver pass,
    // which `check_source` does not, so they are covered by the expansion pass's
    // own unit tests instead.)
    assert_code(
        "struct S:\n    @inject\n    int64 x\n\ndef main():\n    pass\n",
        "ZAP5055",
    );
    // `@cache` is a free-function decorator: misapplied to a struct it is a clean
    // require-target error (ZAP5047), which sema — and so `check_source` — sees.
    assert_code(
        "@cache\nstruct S:\n    int64 x\n\ndef main():\n    pass\n",
        "ZAP5047",
    );
    // `@deprecated` on the wrong target (a struct) — it is a function/method
    // decorator only (I#472, D-0109).
    assert_code(
        "@deprecated\nstruct S:\n    int64 x\n\ndef main():\n    pass\n",
        "ZAP5047",
    );
    // An unknown decorator.
    assert_code("@notathing\ndef main():\n    pass\n", "ZAP5032");
    // A `**` keyword-collector that is not the last parameter (#601).
    assert_code(
        "def f(int64 **kw, int64 a) -> int64:\n    return a\n\ndef main():\n    pass\n",
        "ZAP5121",
    );
    // A `*`-spread of an unknown-length value into a non-variadic callee (#597).
    assert_code(
        "def f(int64 a, int64 b) -> int64:\n    return a + b\n\
         def main():\n    auto xs = [1, 2]\n    auto y = f(*xs)\n",
        "ZAP5122",
    );
    // A `**`-merge of a non-dict operand in a dict display (#598).
    assert_code(
        "def main():\n    list[int64] xs = [1]\n    auto d = {\"a\": 1, **xs}\n",
        "ZAP5123",
    );
    // A `@scoped` struct missing its context-manager methods (#876, D-0159).
    assert_code(
        "@scoped\nstruct Arena:\n    int64 id = 0\n\ndef main():\n    pass\n",
        "ZAP5127",
    );
    // A detached construction of a `@scoped` value — not bound by `with` (#876).
    assert_code(
        "@scoped\nstruct Arena:\n    int64 id = 0\n    \
         def __enter__(self) -> Arena:\n        return self\n    \
         def __exit__(self):\n        pass\n\
         def main():\n    auto a = Arena()\n",
        "ZAP5128",
    );
    // A `with`-bound `@scoped` value escaping its scope (returned) (#876).
    assert_code(
        "@scoped\nstruct Arena:\n    int64 id = 0\n    \
         def __enter__(self) -> Arena:\n        return self\n    \
         def __exit__(self):\n        pass\n\
         def make() -> Arena:\n    with Arena() as a:\n        return a\n\
         def main():\n    pass\n",
        "ZAP5129",
    );
}

/// Package-defined exception families (#1278): the `@exception` marker and its
/// optional `@exception(Parent)` family edge, plus the raiseability gate. The
/// codes pinned here are the whole diagnostic surface of the feature.
#[test]
fn exception_family_codes() {
    // Malformed `@exception` application: more than one positional parent arg.
    assert_code(
        "@exception(a, b)\nstruct E:\n    int64 x = 0\n\ndef main():\n    pass\n",
        "ZAP5136",
    );
    // `@exception(Parent)` where Parent is a declared type but not `@exception`.
    assert_code(
        "struct Plain:\n    int64 x = 0\n\n@exception(Plain)\nstruct E:\n    \
         int64 x = 0\n\ndef main():\n    pass\n",
        "ZAP5137",
    );
    // A cycle in the family forest (`A`'s parent `B`, `B`'s parent `A`).
    assert_code(
        "@exception(B)\nstruct A:\n    int64 x = 0\n\n@exception(A)\nstruct B:\n    \
         int64 x = 0\n\ndef main():\n    pass\n",
        "ZAP5138",
    );
    // Raising a plain (non-`@exception`) struct — the closed footgun.
    assert_code(
        "struct Point:\n    int64 x = 0\n\ndef main():\n    raise Point()\n",
        "ZAP5139",
    );
    // Catching a plain (non-`@exception`) struct.
    assert_code(
        "struct Point:\n    int64 x = 0\n\ndef main():\n    try:\n        pass\n    \
         except Point:\n        pass\n",
        "ZAP5140",
    );

    // Accept-case: a well-formed family raises and catches cleanly — none of the
    // feature's diagnostics fire.
    let ok = "@exception\nstruct Base:\n    int64 x = 0\n\n@exception(Base)\nstruct Child:\n    \
              int64 x = 0\n\ndef main():\n    try:\n        raise Child()\n    \
              except Base:\n        pass\n";
    for code in ["ZAP5136", "ZAP5137", "ZAP5138", "ZAP5139", "ZAP5140"] {
        assert_no_code(ok, code);
    }
}

/// Assert that checking `src` does NOT surface a diagnostic with `code` — used
/// for the arena accept-cases, which must type-check without tripping the
/// leaf-only (`ZAP5130`) or arena-escape (`ZAP5131`) rules.
#[track_caller]
fn assert_no_code(src: &str, code: &str) {
    let r = check_source(src);
    let got: Vec<String> = r.diagnostics.iter().map(|d| d.code.to_string()).collect();
    assert!(
        !got.iter().any(|c| c == code),
        "did NOT expect {code} for source {src:?}, got {got:?}"
    );
}

/// Arena v1 (#553, D-0158) surface + safety diagnostics. The built-in `arena` is a
/// `@scoped` resource, so its handle reuses ZAP5127/5128/5129; the arena-specific
/// rules are leaf-only (ZAP5130) and arena-value escape (ZAP5131).
#[test]
fn arena_codes() {
    // --- reject ---------------------------------------------------------------
    // Detached construction — not bound by `with` (reuses the @scoped rule).
    assert_code("def main():\n    auto a = arena()\n", "ZAP5128");
    // Returning an arena-allocated value out of its block — a use-after-free.
    assert_code(
        "def leak() -> bytes:\n    with arena() as a:\n        auto buf = a.alloc(bytes, 16)\n        return buf\n\
         def main():\n    pass\n",
        "ZAP5131",
    );
    // Storing an arena-allocated value into a longer-lived slot (element store).
    assert_code(
        "def main():\n    list[bytes] g = []\n    with arena() as a:\n        auto buf = a.alloc(bytes, 8)\n        g[0] = buf\n",
        "ZAP5131",
    );
    // Carrying an arena value across a `spawn` (coroutine-local, D-0158 dec. 7).
    assert_code(
        "def worker(bytes b):\n    pass\n\
         def main():\n    with arena() as a:\n        auto buf = a.alloc(bytes, 8)\n        spawn worker(buf)\n",
        "ZAP5131",
    );
    // A non-leaf (pointer-holding) arena object — leaf-only v1.
    assert_code(
        "struct Node:\n    str name\n\
         def main():\n    with arena() as a:\n        auto n = a.new(Node, \"x\")\n",
        "ZAP5130",
    );
    // Malformed allocation: wrong argument shape.
    assert_code(
        "def main():\n    with arena() as a:\n        auto buf = a.alloc(bytes)\n",
        "ZAP5130",
    );
    // `alloc` of a non-`bytes` element (typed numeric arrays are a v1 follow-up).
    assert_code(
        "def main():\n    with arena() as a:\n        auto buf = a.alloc(int32, 8)\n",
        "ZAP5130",
    );
    // A statically-known `size > cap` is a compile error (#553).
    assert_code(
        "def main():\n    with arena(size=131072, cap=65536) as a:\n        pass\n",
        "ZAP5132",
    );
    // …including through constant folding of the `1<<20` / `64*1024` idioms.
    assert_code(
        "def main():\n    with arena(size=1 << 20, cap=64 * 1024) as a:\n        pass\n",
        "ZAP5132",
    );

    // --- accept (must NOT trip the arena rules) -------------------------------
    // With-bound leaf byte-buffer alloc, then a plain-call borrow.
    assert_no_code(
        "def use_buf(bytes b) -> int64:\n    return len(b)\n\
         def main():\n    with arena(size=1024) as a:\n        auto buf = a.alloc(bytes, 16)\n        auto n = use_buf(buf)\n",
        "ZAP5131",
    );
    // A single leaf struct via `a.new(T, ...)`.
    assert_no_code(
        "struct Pt:\n    int64 x\n    int64 y\n\
         def main():\n    with arena() as a:\n        auto p = a.new(Pt, 1, 2)\n",
        "ZAP5130",
    );
    // A constant `size <= cap` is well-formed (no ZAP5132); `1<<16` <= `1<<20`.
    assert_no_code(
        "def main():\n    with arena(size=1 << 16, cap=1 << 20) as a:\n        pass\n",
        "ZAP5132",
    );
}

/// Borrowed-bytes `strview` affine soundness (#1082, discussion #1097; ZAP5135).
/// A `strview` is a non-owning view valid only while its backing lives, so it may
/// not be stored anywhere that could outlive the backing's frame. These are the
/// walled-garden negative cases: each must be a compile error, and each
/// corresponds to one bullet of the issue's "Cannot borrow (must own)" list.
#[test]
fn strview_affine_codes() {
    // --- reject (ZAP5135) -----------------------------------------------------
    // A strview as a STRUCT FIELD type — would outlive the view's backing.
    assert_code(
        "struct Box:\n    strview v\ndef main():\n    pass\n",
        "ZAP5135",
    );
    // A strview as a FUNCTION RETURN type (v1 forbids cross-frame return).
    assert_code(
        "def first(str s) -> strview:\n    return s.view(0, 1)\ndef main():\n    pass\n",
        "ZAP5135",
    );
    // A strview as a MODULE-GLOBAL type.
    assert_code(
        "strview g = \"x\".view()\ndef main():\n    pass\n",
        "ZAP5135",
    );
    // A strview nested in a COLLECTION element type (list / dict value).
    assert_code("def main():\n    list[strview] xs = []\n", "ZAP5135");
    assert_code("def main():\n    dict[str, strview] d = {}\n", "ZAP5135");

    // --- accept (no ZAP5135) --------------------------------------------------
    // An intra-frame local: sound (the view roots its own backing), and a
    // `strview` PARAMETER is the intended read-only borrowing API.
    assert_no_code(
        "def probe(strview v) -> bool:\n    return v.startswith(\"k\")\n\
         def main():\n    str s = \"key\"\n    strview v = s.view()\n    bool b = probe(v)\n",
        "ZAP5135",
    );
    // Materializing with .to_str() escapes the borrow legitimately (owned str).
    assert_no_code(
        "def main() -> str:\n    str s = \"abc\"\n    return s.view(0, 2).to_str()\n",
        "ZAP5135",
    );
}

/// The borrowed `bytesview` (#1082 follow-up) is affine under the identical rule
/// (`ZAP5135`) as `strview` — it may not escape its backing's frame.
#[test]
fn bytesview_affine_codes() {
    // reject: a bytesview as a struct field / return / collection-element type.
    assert_code(
        "struct Box:\n    bytesview v\ndef main():\n    pass\n",
        "ZAP5135",
    );
    assert_code(
        "def first(bytes s) -> bytesview:\n    return s.view(0, 1)\ndef main():\n    pass\n",
        "ZAP5135",
    );
    assert_code("def main():\n    list[bytesview] xs = []\n", "ZAP5135");
    // accept: an intra-frame local and a bytesview parameter are sound.
    assert_no_code(
        "def probe(bytesview v) -> bool:\n    return v.startswith(bytes([1]))\n\
         def main():\n    bytes b = bytes([1, 2])\n    bytesview v = b.view()\n    \
         bool ok = probe(v)\n",
        "ZAP5135",
    );
}

#[test]
fn eq_none_on_value_type_is_rejected() {
    // `== None` / `!= None` is consistent with `is None` (SPEC §5, #374/B3): a
    // never-None value type can never be None, so this is a ZAP4074 error rather
    // than a silent sema pass that then fails in lowering.
    assert_code(
        "def main():\n    int64 n = 5\n    print(n == None)\n",
        "ZAP4074",
    );
    assert_code(
        "def main():\n    int64 n = 5\n    print(n != None)\n",
        "ZAP4074",
    );
}

#[test]
fn augmented_assign_to_const_is_rejected() {
    // A `const` binding cannot be reassigned, including via an augmented
    // assignment (D-0003; #374 walled garden, operators).
    assert_code(
        "def main():\n    const int64 c = 5\n    c += 1\n",
        "ZAP3017",
    );
}

#[test]
fn call_argument_matrix_is_rejected() {
    // The compile-time argument-checking matrix (#374 walled garden, §B-6).
    // Each mistake is caught at compile time with a stable code, so a call that
    // does not match its callee's signature never reaches codegen.
    //
    // Too many / too few positional arguments (arity) -> ZAP4020.
    assert_code(
        "def f(int64 a) -> int64:\n    return a\n\ndef main():\n    print(f(1, 2))\n",
        "ZAP4020",
    );
    assert_code(
        "def f(int64 a, int64 b) -> int64:\n    return a\n\ndef main():\n    print(f(1))\n",
        "ZAP4020",
    );
    // An unexpected keyword argument -> ZAP4023.
    assert_code(
        "def f(int64 a) -> int64:\n    return a\n\ndef main():\n    print(f(z=1))\n",
        "ZAP4023",
    );
    // A keyword-only parameter (after a bare `*`) supplied positionally ->
    // ZAP4029; omitting the required keyword-only param -> ZAP4020 (I#496, D-0095).
    assert_code(
        "def f(str h, *, int64 m) -> int64:\n    return m\n\ndef main():\n    print(f(\"a\", 80))\n",
        "ZAP4029",
    );
    assert_code(
        "def f(str h, *, int64 m) -> int64:\n    return m\n\ndef main():\n    print(f(\"a\"))\n",
        "ZAP4020",
    );
    // A positional-only parameter (before a `/`) supplied by name -> ZAP4034
    // (#602, D10).
    assert_code(
        "def f(int64 a, /) -> int64:\n    return a\n\ndef main():\n    print(f(a=1))\n",
        "ZAP4034",
    );
}

#[test]
fn str_method_limits_are_rejected() {
    // The str method surface is a subset of CPython's (#284 / #971, #374 walled
    // garden). The thin method set (#971, D-0210) LIFTED the former limits —
    // whitespace/`maxsplit` split, char-set `strip`, `count`/`rfind`/`title`/…
    // are now valid (covered at runtime by `tests/golden/string_methods_ext`).
    // These cases pin what is STILL unsupported, so a program relying on an
    // unsupported form fails at compile time with a stable code.
    //
    // replace() takes exactly (old, new) — no `count` argument yet.
    assert_code(
        "def main():\n    print(\"aaa\".replace(\"a\", \"b\", 1))\n",
        "ZAP4025",
    );
    // strip()/lstrip()/rstrip() take at most one (chars) argument.
    assert_code(
        "def main():\n    print(\"xxhi\".strip(\"x\", \"y\"))\n",
        "ZAP4025",
    );
    // split()/rsplit() take at most (sep, maxsplit).
    assert_code(
        "def main():\n    auto p = \"a b c\".split(\" \", 1, 2)\n",
        "ZAP4025",
    );
    // maxsplit must be an int, not a str.
    assert_code(
        "def main():\n    auto p = \"a b\".split(\" \", \"x\")\n",
        "ZAP4024",
    );
    // ljust() needs a width; the width must be an int.
    assert_code("def main():\n    print(\"hi\".ljust())\n", "ZAP4025");
    assert_code("def main():\n    print(\"hi\".ljust(\"x\"))\n", "ZAP4024");
    // partition / rpartition are now implemented (#1029, D-0218) — they return a
    // (str, str, str) tuple. Pin their argument limits: exactly one str separator
    // (wrong arity -> ZAP4025, a non-str separator -> ZAP4024).
    assert_code(
        "def main():\n    auto t = \"a-b\".partition(\"-\", \"x\")\n",
        "ZAP4025",
    );
    assert_code(
        "def main():\n    auto t = \"a-b\".rpartition(5)\n",
        "ZAP4024",
    );
    // translate / format_map / expandtabs are now implemented (#1034, D-0214);
    // pin their argument limits rather than treating them as unknown methods.
    // translate / format_map want a dict[str, str], not a str:
    assert_code(
        "def main():\n    print(\"a\".translate(\"x\"))\n",
        "ZAP4024",
    );
    assert_code(
        "def main():\n    print(\"{x}\".format_map(\"x\"))\n",
        "ZAP4024",
    );
    // translate / format_map take exactly one argument:
    assert_code(
        "def main():\n    auto d = {\"a\": \"b\"}\n    print(\"a\".translate(d, d))\n",
        "ZAP4025",
    );
    // expandtabs takes an optional int tabsize — at most one, int-typed:
    assert_code(
        "def main():\n    print(\"a\".expandtabs(1, 2))\n",
        "ZAP4025",
    );
    assert_code(
        "def main():\n    print(\"a\".expandtabs(\"x\"))\n",
        "ZAP4024",
    );
}

#[test]
fn str_times_int_is_rejected() {
    // `str * int` repetition is a FEATURE-GAP (not in SPEC §8 yet, #374): it is
    // rejected at compile time rather than silently supported.
    assert_code("def main():\n    print(\"ab\" * 3)\n", "ZAP4012");
}

#[test]
fn json_payload_carries_the_code() {
    // The structured surface agents consume must expose the code as a field.
    let r = check_source("def main():\n    x = 1 +\n");
    let json = result_to_json(&r).to_string();
    assert!(
        json.contains("\"code\":\"ZAP2014\""),
        "JSON payload is missing the code field: {json}"
    );
}

#[test]
fn clean_program_has_no_diagnostics() {
    let r = check_source("def main():\n    print(1)\n");
    assert!(r.ok, "unexpected diagnostics: {:?}", r.diagnostics);
}

#[test]
fn hash_dunder_and_hash_combine_misuse_is_rejected() {
    // `str.__hash__()` (and every intrinsic `__hash__()`) takes no arguments
    // (#1064) — a stray argument is the arity error ZAP4025.
    assert_code("def main():\n    x = \"abc\".__hash__(5)\n", "ZAP4025");
    // `hash_combine(acc, h)` takes exactly two arguments ...
    assert_code("def main():\n    x = hash_combine(1)\n", "ZAP4025");
    // ... and both must be int: a str argument is the arg-type error ZAP4024.
    assert_code("def main():\n    x = hash_combine(1, \"b\")\n", "ZAP4024");
    // A zero-arg `str.__hash__()` and `hash_combine(int, int)` are clean.
    let ok = check_source(
        "def main():\n    auto h = \"abc\".__hash__()\n    auto c = hash_combine(h, 5)\n",
    );
    assert!(ok.ok, "unexpected diagnostics: {:?}", ok.diagnostics);
}

#[test]
fn repr_ascii_base_builtin_codes() {
    // repr/ascii (#1384) take exactly one argument (wrong arity -> ZAP4025) and
    // accept the same renderable operand set as str(); there is no operand that
    // str() accepts but repr() rejects, so the arity error is the representative.
    assert_code("def main():\n    x = repr()\n", "ZAP4025");
    assert_code("def main():\n    x = ascii(1, 2)\n", "ZAP4025");

    // bin/oct/hex (#1384) take exactly one *integer* argument. A non-integer is
    // the arg-type error ZAP4024; `bool`/`float` are rejected (no Python
    // int-subclass coercion). Wrong arity is ZAP4025.
    assert_code("def main():\n    x = bin(1.5)\n", "ZAP4024");
    assert_code("def main():\n    x = oct(\"nope\")\n", "ZAP4024");
    assert_code("def main():\n    x = hex(True)\n", "ZAP4024");
    assert_code("def main():\n    x = hex(1, 2)\n", "ZAP4025");
    assert_code("def main():\n    x = bin()\n", "ZAP4025");

    // The well-typed forms are clean: repr/ascii over a renderable, and
    // bin/oct/hex over every integer kind (int*/uint*/bigint).
    let ok = check_source(
        "def main():\n\
         \x20   str a = repr(\"hi\")\n\
         \x20   str b = ascii([1, 2])\n\
         \x20   str c = bin(10)\n\
         \x20   str d = oct(uint32(8))\n\
         \x20   str e = hex(bigint(255))\n\
         \x20   int64 lo = -9223372036854775807 - 1\n\
         \x20   str f = hex(lo)\n",
    );
    assert!(ok.ok, "unexpected diagnostics: {:?}", ok.diagnostics);
}

#[test]
fn open_builtin_codes() {
    // open(path[, mode]) (#1388) is prelude sugar over io.open: one or two
    // positional str arguments. Wrong arity -> ZAP4025, a non-str argument ->
    // ZAP4024, and a keyword argument (it is positional) -> ZAP4023.
    assert_code("def main():\n    auto f = open()\n", "ZAP4025");
    assert_code(
        "def main():\n    auto f = open(\"a\", \"w\", \"x\")\n",
        "ZAP4025",
    );
    assert_code("def main():\n    auto f = open(1)\n", "ZAP4024");
    assert_code("def main():\n    auto f = open(\"a\", 2)\n", "ZAP4024");
    assert_code(
        "def main():\n    auto f = open(\"a\", mode=\"w\")\n",
        "ZAP4023",
    );
    // The well-typed forms are clean: default mode and an explicit mode.
    let ok = check_source(
        "import io\n\
         def main():\n\
         \x20   auto a = open(\"data.txt\")\n\
         \x20   io.close_fd(a)\n\
         \x20   auto b = open(\"out.log\", \"w\")\n\
         \x20   io.close_fd(b)\n",
    );
    assert!(ok.ok, "unexpected diagnostics: {:?}", ok.diagnostics);
}

#[test]
fn overload_parity_a_codes() {
    // int(value[, base]) (#1458): one or two positional args. Wrong arity or a
    // keyword argument -> ZAP4025; a non-str value or non-int base in the 2-arg
    // form -> ZAP4024.
    assert_code("def main():\n    auto n = int()\n", "ZAP4025");
    assert_code("def main():\n    auto n = int(\"a\", 2, 3)\n", "ZAP4025");
    assert_code("def main():\n    auto n = int(\"a\", base=16)\n", "ZAP4025");
    assert_code("def main():\n    auto n = int(10, 2)\n", "ZAP4024");
    assert_code("def main():\n    auto n = int(\"10\", \"x\")\n", "ZAP4024");

    // bytes(int) accepts an integer count; a float is still the arg-type error.
    assert_code("def main():\n    auto b = bytes(1.5)\n", "ZAP4024");

    // print() formatting kwargs: an unknown keyword -> ZAP4023; a wrong-typed
    // sep=/file= -> ZAP4024.
    assert_code("def main():\n    print(\"x\", color=\"r\")\n", "ZAP4023");
    assert_code("def main():\n    print(\"x\", sep=5)\n", "ZAP4024");
    assert_code("def main():\n    print(\"x\", file=\"out\")\n", "ZAP4024");

    // The well-typed forms are clean.
    let ok = check_source(
        "def main():\n\
         \x20   int64 a = int(\"ff\", 16)\n\
         \x20   int64 b = int(\"42\")\n\
         \x20   bytes z = bytes(4)\n\
         \x20   bytes y = bytes(\"hi\")\n\
         \x20   print(\"a\", \"b\", sep=\", \", end=\"\", flush=True)\n",
    );
    assert!(ok.ok, "unexpected diagnostics: {:?}", ok.diagnostics);
}

#[test]
fn str_decode_codes() {
    // str(bytes, encoding[, errors]) (#1459): one to three positional args. A
    // keyword argument or wrong arity -> ZAP4025; the decode form requires a
    // bytes value and str encoding/errors -> ZAP4024 otherwise.
    assert_code(
        "def main():\n    auto s = str(b\"x\", \"u\", \"strict\", \"x\")\n",
        "ZAP4025",
    );
    assert_code(
        "def main():\n    auto s = str(b\"x\", encoding=\"utf-8\")\n",
        "ZAP4025",
    );
    assert_code("def main():\n    auto s = str(5, \"utf-8\")\n", "ZAP4024");
    assert_code("def main():\n    auto s = str(b\"x\", 5)\n", "ZAP4024");

    // The well-typed forms are clean: 1-arg render, and the 2-/3-arg decode.
    let ok = check_source(
        "def main():\n\
         \x20   str a = str(42)\n\
         \x20   str b = str(b\"hi\", \"utf-8\")\n\
         \x20   str c = str(b\"hi\", \"ascii\", \"replace\")\n\
         \x20   str d = str(b\"hi\", \"latin-1\")\n",
    );
    assert!(ok.ok, "unexpected diagnostics: {:?}", ok.diagnostics);
}

#[test]
fn list_dict_ctor_codes() {
    // list(iterable) (#1460): exactly one positional arg (ZAP4025 otherwise); a
    // non-iterable operand is ZAP4070.
    assert_code("def main():\n    auto l = list()\n", "ZAP4025");
    assert_code("def main():\n    auto l = list([1], [2])\n", "ZAP4025");
    assert_code("def main():\n    auto l = list(5)\n", "ZAP4070");

    // dict(...) (#1460): empty / >1 positional / mixing positional + keyword ->
    // ZAP4025; a positional iterable whose element is not a 2-tuple -> ZAP4024.
    assert_code("def main():\n    auto d = dict()\n", "ZAP4025");
    assert_code(
        "def main():\n    auto d = dict([(1, 2)], [(3, 4)])\n",
        "ZAP4025",
    );
    assert_code("def main():\n    auto d = dict([1, 2, 3])\n", "ZAP4024");

    // The well-typed forms are clean.
    let ok = check_source(
        "def main():\n\
         \x20   list[int64] a = list([1, 2, 3])\n\
         \x20   list[char] b = list(\"ab\")\n\
         \x20   list[int64] r = list(range(2))\n\
         \x20   dict[int64, str] p = dict([(1, \"a\"), (2, \"b\")])\n\
         \x20   dict[str, int64] k = dict(x=1, y=2)\n\
         \x20   dict[str, int64] c = dict(k)\n",
    );
    assert!(ok.ok, "unexpected diagnostics: {:?}", ok.diagnostics);
}

#[test]
fn vartuple_codes() {
    // tuple(iterable) (#1473): exactly one positional arg (ZAP4025 otherwise); a
    // non-iterable operand is ZAP4070.
    assert_code("def main():\n    auto t = tuple()\n", "ZAP4025");
    assert_code("def main():\n    auto t = tuple([1], [2])\n", "ZAP4025");
    assert_code("def main():\n    auto t = tuple(5)\n", "ZAP4070");

    // The well-typed forms are clean: tuple(iterable), the typed/empty
    // tuple[T]() / tuple[T](iterable), a tuple[T] annotation, indexing, and len.
    let ok = check_source(
        "def main():\n\
         \x20   auto a = tuple([1, 2, 3])\n\
         \x20   auto b = tuple(\"ab\")\n\
         \x20   auto c = tuple(range(2))\n\
         \x20   tuple[int64] d = tuple[int64]()\n\
         \x20   tuple[int64] e = tuple[int64]([1, 2])\n\
         \x20   tuple[int64] f = tuple([5, 6])\n\
         \x20   int64 g = f[0]\n\
         \x20   int64 n = len(a)\n\
         \x20   for x in a:\n\
         \x20       print(x)\n",
    );
    assert!(ok.ok, "unexpected diagnostics: {:?}", ok.diagnostics);
}

#[test]
fn vartuple_key_codes() {
    // A tuple[T] is hashable (iff its element is a scalar/str) and can be a dict
    // key / set member (#1474). These well-typed uses are clean.
    let ok = check_source(
        "def main():\n\
         \x20   dict[tuple[int64], str] d = dict[tuple[int64], str]()\n\
         \x20   d[tuple([1, 2])] = \"a\"\n\
         \x20   str v = d[tuple([1, 2])]\n\
         \x20   bool has = tuple([1, 2]) in d\n\
         \x20   auto s = set([tuple([1, 2]), tuple([3, 4])])\n\
         \x20   bool m = tuple([3, 4]) in s\n\
         \x20   dict[tuple[str], int64] w = dict[tuple[str], int64]()\n\
         \x20   w[tuple([\"x\", \"y\"])] = 7\n",
    );
    assert!(ok.ok, "unexpected diagnostics: {:?}", ok.diagnostics);
}

#[test]
fn vartuple_ops_codes() {
    // tuple[T] ordering types as bool; concat and slice type as tuple[T] (#1475).
    let ok = check_source(
        "def main():\n\
         \x20   auto a = tuple([1, 2, 3])\n\
         \x20   auto b = tuple([4, 5])\n\
         \x20   bool lt = a < b\n\
         \x20   bool le = a <= b\n\
         \x20   bool gt = a > b\n\
         \x20   tuple[int64] cat = a + b\n\
         \x20   tuple[int64] sl = a[0:2]\n\
         \x20   tuple[int64] rev = a[::-1]\n\
         \x20   int64 n = len(cat)\n",
    );
    assert!(ok.ok, "unexpected diagnostics: {:?}", ok.diagnostics);
}

#[test]
fn generic_interface_value_position_codes() {
    // Generic-interface conformance is enforced at value positions (#1428):
    // `IntBox` conforms to `Container[int64]`, so the call is clean ...
    const PREFIX: &str = "interface Container[E]:\n    def get(self) -> E\n\n\
        struct IntBox:\n    int64 v\n\
        \x20   def get(self) -> int64:\n        return self.v\n\n";
    let ok = check_source(&format!(
        "{PREFIX}def take(Container[int64] c):\n    print(1)\n\n\
         def main():\n    take(IntBox())\n"
    ));
    assert!(ok.ok, "unexpected diagnostics: {:?}", ok.diagnostics);
    // ... and a wrong instantiation `Container[str]` is the argument-type error
    // ZAP4021 (the value position is now checked against the real `[Arg]`).
    assert_code(
        &format!(
            "{PREFIX}def take(Container[str] c):\n    print(1)\n\n\
             def main():\n    take(IntBox())\n"
        ),
        "ZAP4021",
    );
}

#[test]
fn iter_next_builtin_codes() {
    // `iter()`/`next()` (#1382 phase 2) type-check over a protocol iterator ...
    const CNT: &str = "struct Count:\n    int64 i\n\
        \x20   def __iter__(self) -> Count:\n        return self\n\
        \x20   def __next__(self) -> (int64, bool):\n        return (self.i, True)\n\n";
    let ok = check_source(&format!(
        "{CNT}def main():\n    auto it = iter(Count(i=0))\n    \
         int64 a = next(it)\n    int64 b = next(it, -1)\n    print(a)\n    print(b)\n"
    ));
    assert!(ok.ok, "unexpected diagnostics: {:?}", ok.diagnostics);
    // ... reject iter() over a built-in container (reified iterators are phase 3)
    assert_code("def main():\n    auto it = iter([1, 2, 3])\n", "ZAP4024");
    // ... reject next() of a non-iterator value ...
    assert_code("def main():\n    auto x = next(5)\n", "ZAP4024");
    // ... and a next() default whose type does not match the element type.
    assert_code(
        &format!(
            "{CNT}def main():\n    auto it = iter(Count(i=0))\n    auto x = next(it, \"no\")\n"
        ),
        "ZAP4024",
    );
}
