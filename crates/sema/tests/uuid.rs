//! `uuid` module — sema shape/arity/type checks (#975, D-0232/D-0233).
//!
//! Exhaustive type-checking for the UUID surface: the generators and `parse`
//! return the prelude `Uuid` value struct; the accessor methods (`str(u)` via
//! the Display protocol, `hex`/`urn`/`to_bytes`/`version`/`variant`) return
//! their respective types; `nanoid` returns `str` and dispatches by arity + type;
//! and the argument type/arity diagnostics. Host-only (parse + sema), no LLVM.

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

fn prog(body: &str) -> String {
    format!("def f():\n{body}\n")
}

// ---------------------------------------------------------------------------
// Return types — generators / parse / constants -> Uuid. Annotated locals force
// the check.
// ---------------------------------------------------------------------------

#[test]
fn generators_return_uuid() {
    assert_ok(&prog("    Uuid u = uuid.uuid4()\n"));
    assert_ok(&prog("    Uuid u = uuid.uuid7()\n"));
    assert_ok(&prog("    Uuid u = uuid.uuid1()\n"));
    assert_ok(&prog("    Uuid u = uuid.nil()\n"));
    assert_ok(&prog("    Uuid u = uuid.max()\n"));
}

#[test]
fn namespace_constants_return_uuid() {
    assert_ok(&prog("    Uuid u = uuid.namespace_dns()\n"));
    assert_ok(&prog("    Uuid u = uuid.namespace_url()\n"));
    assert_ok(&prog("    Uuid u = uuid.namespace_oid()\n"));
    assert_ok(&prog("    Uuid u = uuid.namespace_x500()\n"));
}

#[test]
fn namespaced_and_parse_return_uuid() {
    assert_ok(&prog(
        "    Uuid u = uuid.uuid3(uuid.namespace_dns(), \"example.com\")\n",
    ));
    assert_ok(&prog(
        "    Uuid u = uuid.uuid5(uuid.namespace_dns(), \"example.com\")\n",
    ));
    assert_ok(&prog(
        "    Uuid u = uuid.parse(\"6ba7b810-9dad-11d1-80b4-00c04fd430c8\")\n",
    ));
    // A bytes name is accepted too (str/bytes share the ZapStr layout).
    assert_ok(&prog(
        "    Uuid u = uuid.uuid5(uuid.namespace_dns(), b\"example.com\")\n",
    ));
}

// ---------------------------------------------------------------------------
// Accessor methods — str(u) via Display; hex/urn -> str; to_bytes -> bytes;
// version/variant -> int64.
// ---------------------------------------------------------------------------

#[test]
fn accessors_return_types() {
    assert_ok(&prog("    auto u = uuid.uuid4()\n    str s = str(u)\n"));
    assert_ok(&prog("    auto u = uuid.uuid4()\n    str h = u.hex()\n"));
    assert_ok(&prog("    auto u = uuid.uuid4()\n    str n = u.urn()\n"));
    assert_ok(&prog(
        "    auto u = uuid.uuid4()\n    bytes b = u.to_bytes()\n",
    ));
    assert_ok(&prog(
        "    auto u = uuid.uuid4()\n    int64 v = u.version()\n",
    ));
    assert_ok(&prog(
        "    auto u = uuid.uuid4()\n    int64 r = u.variant()\n",
    ));
    // Display protocol: a Uuid interpolates / prints.
    assert_ok(&prog("    auto u = uuid.uuid4()\n    print(f\"id={u}\")\n"));
}

#[test]
fn uuid_is_equatable_and_hashable() {
    // A value struct auto-derives Eq + Hash (D-0139): `==` and dict/set keys.
    assert_ok(&prog(
        "    auto a = uuid.nil()\n    auto b = uuid.nil()\n    bool e = a == b\n",
    ));
    assert_ok(&prog(
        "    dict[Uuid, int64] m = {}\n    m[uuid.uuid4()] = 1\n",
    ));
    assert_ok(&prog("    set[Uuid] s = {uuid.uuid4()}\n"));
}

// ---------------------------------------------------------------------------
// NanoID — str result; 0 / 1 (size) / 2 (alphabet, size) forms.
// ---------------------------------------------------------------------------

#[test]
fn nanoid_arities_return_str() {
    assert_ok(&prog("    str s = uuid.nanoid()\n"));
    assert_ok(&prog("    str s = uuid.nanoid(10)\n"));
    assert_ok(&prog("    str s = uuid.nanoid(\"abcdef\", 12)\n"));
}

// ---------------------------------------------------------------------------
// Arity diagnostics (ZAP4025).
// ---------------------------------------------------------------------------

#[test]
fn arity_errors() {
    assert_code(&prog("    auto u = uuid.uuid4(1)\n"), "ZAP4025");
    assert_code(&prog("    auto u = uuid.nil(1)\n"), "ZAP4025");
    assert_code(
        &prog("    auto u = uuid.uuid5(uuid.namespace_dns())\n"),
        "ZAP4025",
    );
    assert_code(&prog("    auto u = uuid.parse()\n"), "ZAP4025");
    assert_code(
        &prog("    auto s = uuid.nanoid(\"abc\", 1, 2)\n"),
        "ZAP4025",
    );
}

// ---------------------------------------------------------------------------
// Argument type diagnostics (ZAP4024).
// ---------------------------------------------------------------------------

#[test]
fn type_errors() {
    // parse wants a str/bytes.
    assert_code(&prog("    auto u = uuid.parse(42)\n"), "ZAP4024");
    // uuid5's namespace must be a Uuid, not a bare string.
    assert_code(
        &prog("    auto u = uuid.uuid5(\"not-a-uuid\", \"name\")\n"),
        "ZAP4024",
    );
    // nanoid(size) wants an int.
    assert_code(&prog("    auto s = uuid.nanoid(\"nope\")\n"), "ZAP4024");
    // nanoid(alphabet, size): alphabet str, size int.
    assert_code(&prog("    auto s = uuid.nanoid(42, 10)\n"), "ZAP4024");
    assert_code(
        &prog("    auto s = uuid.nanoid(\"abc\", \"ten\")\n"),
        "ZAP4024",
    );
}

#[test]
fn unknown_function_errors() {
    assert_code(&prog("    auto u = uuid.uuid6()\n"), "ZAP3003");
}
