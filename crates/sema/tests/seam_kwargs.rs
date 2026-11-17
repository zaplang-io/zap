//! Seam calling convention: keyword / default argument binding for built-in
//! module functions (#998, epic #967).
//!
//! Built-in (seam) module functions used to be positional-only — they tolerated
//! keyword syntax but ignored the names, so a misspelled keyword on a stdlib
//! call was *silently accepted* (a correctness footgun) and keyword reordering
//! did not work. `zapc_sema::modules::bind_seam_args` gives an opt-in method a
//! Python-shaped calling convention; `zlib`/`gzip` `compress`/`decompress`
//! (and `zlib.crc32`/`adler32`) are the first migrated methods. These tests run
//! on the host (parse + sema), no LLVM.

use zapc_sema::check_str;

/// Sema errors (parse/lex asserted empty) for a program.
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
// Acceptance item 1 — a known keyword binds by name and permits reordering.
// ---------------------------------------------------------------------------

#[test]
fn keyword_binds_by_name() {
    assert_ok(&prog("    auto b = zlib.compress(data=\"hi\", level=9)\n"));
}

#[test]
fn keyword_reordering_works() {
    // Swapped order: the int `level` and the str `data` must still land in the
    // right slots (positionally this would put the int in the data slot).
    assert_ok(&prog("    auto b = zlib.compress(level=9, data=\"hi\")\n"));
}

#[test]
fn positional_and_keyword_mix() {
    assert_ok(&prog("    auto b = zlib.compress(\"hi\", level=9)\n"));
}

// ---------------------------------------------------------------------------
// Acceptance item 2 — an unknown keyword is rejected (the silent-accept footgun).
// ---------------------------------------------------------------------------

#[test]
fn unknown_keyword_is_rejected() {
    assert_code(
        &prog("    auto b = zlib.compress(\"hi\", bogus=9)\n"),
        "ZAP4023",
    );
}

#[test]
fn unknown_keyword_on_decompress_is_rejected() {
    assert_code(
        &prog("    auto b = zlib.decompress(payload=\"hi\")\n"),
        "ZAP4023",
    );
}

#[test]
fn unknown_keyword_on_crc32_is_rejected() {
    assert_code(&prog("    auto n = zlib.crc32(buf=\"hi\")\n"), "ZAP4023");
}

// ---------------------------------------------------------------------------
// Acceptance item 3 — a defaulted parameter can be omitted and filled by name.
// ---------------------------------------------------------------------------

#[test]
fn defaulted_level_omitted_by_name() {
    // `data` supplied by name, optional `level` omitted entirely.
    assert_ok(&prog("    auto b = zlib.compress(data=\"hi\")\n"));
}

// ---------------------------------------------------------------------------
// Binding-error diagnostics.
// ---------------------------------------------------------------------------

#[test]
fn duplicate_value_for_parameter_is_rejected() {
    // `data` supplied positionally and again by keyword.
    assert_code(
        &prog("    auto b = zlib.compress(\"hi\", data=\"bye\")\n"),
        "ZAP4023",
    );
}

#[test]
fn missing_earlier_parameter_is_rejected() {
    // `level` by name with `data` omitted → a hole, not a silent shift of
    // `level` into the `data` slot.
    assert_code(&prog("    auto b = zlib.compress(level=9)\n"), "ZAP4025");
}

// ---------------------------------------------------------------------------
// gzip shares the migrated check path.
// ---------------------------------------------------------------------------

#[test]
fn gzip_keyword_binding_shared() {
    assert_ok(&prog("    auto b = gzip.compress(data=\"hi\", level=6)\n"));
}

#[test]
fn gzip_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto b = gzip.compress(\"hi\", bogus=1)\n"),
        "ZAP4023",
    );
}

// ---------------------------------------------------------------------------
// Regression — plain positional calls are unchanged.
// ---------------------------------------------------------------------------

#[test]
fn positional_calls_unchanged() {
    assert_ok(&prog("    auto a = zlib.compress(\"hi\")\n"));
    assert_ok(&prog("    auto b = zlib.compress(\"hi\", 9)\n"));
    assert_ok(&prog("    auto c = zlib.decompress(\"hi\")\n"));
    assert_ok(&prog("    auto d = zlib.crc32(\"hi\")\n"));
}

#[test]
fn wrong_type_keyword_still_type_checks() {
    // `level` bound by name but a str → the existing level type check (ZAP4024)
    // still fires on the normalized argument.
    assert_code(
        &prog("    auto b = zlib.compress(\"hi\", level=\"nine\")\n"),
        "ZAP4024",
    );
}

// ---------------------------------------------------------------------------
// The framework generalizes: `http` is driven by the same central registry
// (#998, D-0204). The client verbs' optional trailing `headers` dict is the
// former "n or n+1 args" overload, now a named optional parameter.
// ---------------------------------------------------------------------------

#[test]
fn http_get_keyword_headers() {
    assert_ok(&prog(
        "    auto h = {\"a\": \"b\"}\n    auto r = http.get(\"http://x\", headers=h)\n",
    ));
}

#[test]
fn http_get_url_by_name_headers_omitted() {
    assert_ok(&prog("    auto r = http.get(url=\"http://x\")\n"));
}

#[test]
fn http_get_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto r = http.get(\"http://x\", timeout=5)\n"),
        "ZAP4023",
    );
}

#[test]
fn http_post_reordered_keywords() {
    assert_ok(&prog(
        "    auto r = http.post(body=\"n=zap\", url=\"http://x\")\n",
    ));
}

#[test]
fn http_request_keywords_and_optional_headers() {
    assert_ok(&prog(
        "    auto h = {\"a\": \"b\"}\n    auto r = http.request(method=\"GET\", url=\"http://x\", body=\"\", headers=h)\n",
    ));
}

#[test]
fn http_positional_calls_unchanged() {
    assert_ok(&prog("    auto r = http.get(\"http://x\")\n"));
    assert_ok(&prog("    auto r = http.post(\"http://x\", \"n=zap\")\n"));
}

// ---------------------------------------------------------------------------
// datetime — timestamp conversions + format/parse (#99/#338).
// ---------------------------------------------------------------------------

#[test]
fn datetime_strftime_keyword_and_reordered() {
    assert_ok(&prog(
        "    auto d = datetime.now()\n    auto s = datetime.strftime(dt=d, fmt=\"%Y\")\n",
    ));
    assert_ok(&prog(
        "    auto d = datetime.now()\n    auto s = datetime.strftime(fmt=\"%Y\", dt=d)\n",
    ));
}

#[test]
fn datetime_format_optional_fmt_by_name() {
    // `format(dt)` is ISO-8601; the strftime `fmt` is the trailing optional.
    assert_ok(&prog(
        "    auto d = datetime.now()\n    auto s = datetime.format(dt=d)\n",
    ));
}

#[test]
fn datetime_from_timestamp_keyword() {
    assert_ok(&prog("    auto d = datetime.from_timestamp(timestamp=0)\n"));
}

#[test]
fn datetime_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto d = datetime.now()\n    auto s = datetime.strftime(dt=d, bogus=\"x\")\n"),
        "ZAP4023",
    );
}

#[test]
fn datetime_positional_unchanged() {
    assert_ok(&prog(
        "    auto d = datetime.now()\n    auto s = datetime.strftime(d, \"%Y\")\n",
    ));
}

// ---------------------------------------------------------------------------
// random — the non-numpy-overloaded verbs (seed, randint).
// ---------------------------------------------------------------------------

#[test]
fn random_randint_keyword_and_reordered() {
    assert_ok(&prog("    auto n = random.randint(a=1, b=10)\n"));
    assert_ok(&prog("    auto n = random.randint(b=10, a=1)\n"));
}

#[test]
fn random_seed_keyword() {
    assert_ok(&prog("    random.seed(n=42)\n"));
}

#[test]
fn random_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto n = random.randint(1, bogus=10)\n"),
        "ZAP4023",
    );
}

#[test]
fn random_positional_unchanged() {
    assert_ok(&prog("    auto n = random.randint(1, 10)\n"));
}

// ---------------------------------------------------------------------------
// base64 / hex / hash / json / yaml — single-argument encoders/serializers:
// keyword binding is validation (an unknown keyword is now caught).
// ---------------------------------------------------------------------------

#[test]
fn encoders_keyword_data() {
    assert_ok(&prog("    auto a = base64.encode(data=\"hi\")\n"));
    assert_ok(&prog("    auto b = hex.encode(data=\"hi\")\n"));
    assert_ok(&prog("    auto c = hash.crc32c(data=\"hi\")\n"));
}

#[test]
fn encoders_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto a = base64.encode(bogus=\"hi\")\n"),
        "ZAP4023",
    );
    assert_code(&prog("    auto b = hex.encode(bogus=\"hi\")\n"), "ZAP4023");
}

#[test]
fn json_yaml_keyword_and_unknown() {
    // json.dumps/yaml.dump take a JsonValue (produced by loads/load).
    assert_ok(&prog(
        "    auto jv = json.loads(s=\"42\")\n    auto s = json.dumps(v=jv)\n",
    ));
    assert_ok(&prog(
        "    auto jv = yaml.load(s=\"42\")\n    auto y = yaml.dump(v=jv)\n",
    ));
    assert_code(
        &prog("    auto jv = json.loads(\"42\")\n    auto s = json.dumps(value=jv)\n"),
        "ZAP4023",
    );
}

#[test]
fn encoders_positional_unchanged() {
    assert_ok(&prog("    auto a = base64.encode(\"hi\")\n"));
    assert_ok(&prog(
        "    auto jv = json.loads(\"42\")\n    auto s = json.dumps(jv)\n",
    ));
}

// ---------------------------------------------------------------------------
// logging — level emitters + the two-arg `log(level, msg)` (reorder).
// ---------------------------------------------------------------------------

#[test]
fn logging_keyword_and_reorder() {
    assert_ok(&prog("    logging.info(msg=\"hello\")\n"));
    assert_ok(&prog("    logging.log(level=2, msg=\"hello\")\n"));
    assert_ok(&prog("    logging.log(msg=\"hello\", level=2)\n"));
}

#[test]
fn logging_unknown_keyword_rejected() {
    assert_code(&prog("    logging.info(message=\"hello\")\n"), "ZAP4023");
}

#[test]
fn logging_positional_unchanged() {
    assert_ok(&prog("    logging.info(\"hello\")\n"));
    assert_ok(&prog("    logging.log(2, \"hello\")\n"));
}

// ---------------------------------------------------------------------------
// crypto — KDFs / AEAD with their documented trailing optionals (dklen, salt,
// info, aad). The highest-value footgun: a typo'd `dklen`/`iterations` was
// silently ignored before.
// ---------------------------------------------------------------------------

#[test]
fn crypto_pbkdf2_keyword_reorder_and_optional_dklen() {
    assert_ok(&prog(
        "    auto k = crypto.pbkdf2_hmac(hash_name=\"sha256\", password=\"pw\", salt=\"s\", iterations=1000)\n",
    ));
    // reordered, dklen omitted (optional)
    assert_ok(&prog(
        "    auto k = crypto.pbkdf2_hmac(iterations=1000, hash_name=\"sha256\", password=\"pw\", salt=\"s\")\n",
    ));
}

#[test]
fn crypto_scrypt_and_hkdf_optionals() {
    assert_ok(&prog(
        "    auto k = crypto.scrypt(password=\"pw\", salt=\"s\", n=16384, r=8, p=1)\n",
    ));
    assert_ok(&prog(
        "    auto k = crypto.hkdf(hash_name=\"sha256\", ikm=\"k\", length=32)\n",
    ));
}

#[test]
fn crypto_aead_keyword_optional_aad() {
    assert_ok(&prog(
        "    auto c = crypto.aes256gcm_encrypt(key=\"k\", nonce=\"n\", data=\"d\")\n",
    ));
    assert_ok(&prog(
        "    auto c = crypto.chacha20poly1305_encrypt(data=\"d\", key=\"k\", nonce=\"n\")\n",
    ));
}

#[test]
fn crypto_unknown_keyword_rejected() {
    // a typo'd `dklen` is now caught, not silently dropped
    assert_code(
        &prog("    auto k = crypto.pbkdf2_hmac(\"sha256\", \"pw\", \"s\", 1000, dklenght=64)\n"),
        "ZAP4023",
    );
}

#[test]
fn crypto_positional_unchanged() {
    assert_ok(&prog(
        "    auto k = crypto.pbkdf2_hmac(\"sha256\", \"pw\", \"s\", 1000, 64)\n",
    ));
    assert_ok(&prog(
        "    auto b = crypto.constant_time_eq(\"a\", \"b\")\n",
    ));
}

// ---------------------------------------------------------------------------
// os / fs — environment + filesystem helpers.
// ---------------------------------------------------------------------------

#[test]
fn os_setenv_keyword_and_reorder() {
    assert_ok(&prog("    os.setenv(name=\"X\", value=\"1\")\n"));
    assert_ok(&prog("    os.setenv(value=\"1\", name=\"X\")\n"));
}

#[test]
fn os_unknown_keyword_rejected() {
    assert_code(&prog("    auto v = os.getenv(key=\"PATH\")\n"), "ZAP4023");
}

#[test]
fn fs_write_file_keyword_and_unknown() {
    assert_ok(&prog("    fs.write_file(path=\"/tmp/x\", data=\"hi\")\n"));
    assert_code(
        &prog("    fs.write_file(path=\"/tmp/x\", contents=\"hi\")\n"),
        "ZAP4023",
    );
}

#[test]
fn os_fs_positional_unchanged() {
    assert_ok(&prog("    auto v = os.getenv(\"PATH\")\n"));
    assert_ok(&prog("    fs.write_file(\"/tmp/x\", \"hi\")\n"));
}

// ---------------------------------------------------------------------------
// time / process / sys.
// ---------------------------------------------------------------------------

#[test]
fn time_keyword_and_duration_helpers() {
    assert_ok(&prog("    time.sleep(seconds=0.0)\n"));
    assert_ok(&prog(
        "    auto d = time.seconds(n=5)\n    auto s = time.to_seconds(d=d)\n",
    ));
}

#[test]
fn time_unknown_keyword_rejected() {
    assert_code(&prog("    time.sleep(secs=0.0)\n"), "ZAP4023");
}

#[test]
fn process_run_keyword() {
    assert_ok(&prog("    auto r = process.run(cmd=[\"echo\"])\n"));
}

#[test]
fn sys_keyword_and_unknown() {
    assert_ok(&prog("    auto h = sys.cpu_has(name=\"avx2\")\n"));
    assert_code(
        &prog("    auto h = sys.cpu_has(feature=\"avx2\")\n"),
        "ZAP4023",
    );
}

#[test]
fn time_process_sys_positional_unchanged() {
    assert_ok(&prog("    time.sleep(0.0)\n"));
    assert_ok(&prog("    auto r = process.run([\"echo\"])\n"));
    assert_ok(&prog("    auto h = sys.cpu_has(\"avx2\")\n"));
}

// ---------------------------------------------------------------------------
// url / io / signal.
// ---------------------------------------------------------------------------

#[test]
fn url_keyword_and_unknown() {
    assert_ok(&prog("    auto q = url.quote(s=\"a b\")\n"));
    assert_ok(&prog(
        "    auto d = {\"k\": \"v\"}\n    auto q = url.encode_query(d=d)\n",
    ));
    assert_code(&prog("    auto q = url.quote(text=\"a b\")\n"), "ZAP4023");
}

#[test]
fn io_keyword_reorder_and_unknown() {
    assert_ok(&prog(
        "    auto fd = io.open(path=\"/tmp/x\", mode=\"r\")\n",
    ));
    assert_ok(&prog(
        "    auto fd = io.open(mode=\"r\", path=\"/tmp/x\")\n",
    ));
    assert_code(
        &prog("    auto fd = io.open(path=\"/tmp/x\", flags=\"r\")\n"),
        "ZAP4023",
    );
}

#[test]
fn signal_notify_keyword_and_unknown() {
    assert_ok(&prog("    auto ch = signal.notify(sig=Signal.SIGINT)\n"));
    assert_code(
        &prog("    auto ch = signal.notify(signum=Signal.SIGINT)\n"),
        "ZAP4023",
    );
}

#[test]
fn url_io_signal_positional_unchanged() {
    assert_ok(&prog("    auto q = url.quote(\"a b\")\n"));
    assert_ok(&prog("    auto fd = io.open(\"/tmp/x\", \"r\")\n"));
    assert_ok(&prog("    auto ch = signal.notify(Signal.SIGINT)\n"));
}

// ---------------------------------------------------------------------------
// math / net.
// ---------------------------------------------------------------------------

#[test]
fn math_keyword_and_reorder() {
    assert_ok(&prog("    auto y = math.sqrt(x=2.0)\n"));
    assert_ok(&prog("    auto p = math.pow(x=2.0, y=10.0)\n"));
    assert_ok(&prog("    auto p = math.pow(y=10.0, x=2.0)\n"));
    assert_ok(&prog("    auto a = math.atan2(y=1.0, x=1.0)\n"));
}

#[test]
fn math_unknown_keyword_rejected() {
    assert_code(&prog("    auto y = math.sqrt(value=2.0)\n"), "ZAP4023");
}

#[test]
fn net_keyword_reorder_and_unknown() {
    assert_ok(&prog(
        "    auto l = net.listen(host=\"0.0.0.0\", port=8080)\n",
    ));
    assert_ok(&prog(
        "    auto l = net.listen(port=8080, host=\"0.0.0.0\")\n",
    ));
    assert_ok(&prog(
        "    auto c = net.connect(\"h\", 80)\n    auto s = net.recv(conn=c, n=10)\n",
    ));
    assert_code(
        &prog("    auto l = net.listen(host=\"0.0.0.0\", porta=8080)\n"),
        "ZAP4023",
    );
}

#[test]
fn math_net_positional_unchanged() {
    assert_ok(&prog("    auto p = math.pow(2.0, 10.0)\n"));
    assert_ok(&prog("    auto l = net.listen(\"0.0.0.0\", 8080)\n"));
}

// ---------------------------------------------------------------------------
// linalg — LAPACK-backed linear algebra; NumPy names (a / b / n / x / ord).
// The check facet only infers + arity-checks (arrays type loosely), so these
// exercise the central binder, not linalg's own typing.
// ---------------------------------------------------------------------------

const MAT: &str = "    auto m = [[1.0, 2.0], [3.0, 4.0]]\n";

#[test]
fn linalg_keyword_and_reorder() {
    assert_ok(&prog(&format!("{MAT}    auto r = linalg.inv(a=m)\n")));
    assert_ok(&prog(&format!(
        "{MAT}    auto r = linalg.solve(a=m, b=m)\n"
    )));
    // Reordered: solve(b=, a=) must still land a in slot 0.
    assert_ok(&prog(&format!(
        "{MAT}    auto r = linalg.solve(b=m, a=m)\n"
    )));
    assert_ok(&prog(&format!(
        "{MAT}    auto r = linalg.matrix_power(a=m, n=2)\n"
    )));
}

#[test]
fn linalg_optional_ord_by_name() {
    assert_ok(&prog(&format!("{MAT}    auto d = linalg.norm(x=m)\n")));
    assert_ok(&prog(&format!(
        "{MAT}    auto d = linalg.norm(x=m, ord=2)\n"
    )));
}

#[test]
fn linalg_unknown_keyword_rejected() {
    assert_code(
        &prog(&format!("{MAT}    auto r = linalg.inv(matrix=m)\n")),
        "ZAP4023",
    );
}

#[test]
fn linalg_positional_unchanged() {
    assert_ok(&prog(&format!("{MAT}    auto r = linalg.solve(m, m)\n")));
}

// ---------------------------------------------------------------------------
// tls — client/server TLS; ctx / capath / keyfile optional trailing.
// ---------------------------------------------------------------------------

#[test]
fn tls_keyword_and_reorder() {
    assert_ok(&prog("    auto c = tls.connect(host=\"h\", port=443)\n"));
    assert_ok(&prog("    auto c = tls.connect(port=443, host=\"h\")\n"));
    assert_ok(&prog(
        "    auto c = tls.connect(\"h\", 443)\n    auto s = tls.recv(conn=c, n=16)\n",
    ));
    assert_ok(&prog(
        "    auto c = tls.connect(\"h\", 443)\n    auto k = tls.send(conn=c, data=\"x\")\n",
    ));
}

#[test]
fn tls_optional_trailing_by_name() {
    assert_ok(&prog(
        "    auto ctx = tls.SSLContext()\n    tls.load_verify_locations(ctx=ctx, cafile=\"/tmp/ca.pem\")\n",
    ));
    assert_ok(&prog(
        "    auto ctx = tls.SSLContext()\n    tls.load_cert_chain(ctx=ctx, certfile=\"/tmp/c.pem\", keyfile=\"/tmp/k.pem\")\n",
    ));
}

#[test]
fn tls_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto c = tls.connect(host=\"h\", porta=443)\n"),
        "ZAP4023",
    );
}

#[test]
fn tls_positional_unchanged() {
    assert_ok(&prog("    auto c = tls.connect(\"h\", 443)\n"));
}

// ---------------------------------------------------------------------------
// argparse — global singleton parser; add_option/add_int take (name, default).
// ---------------------------------------------------------------------------

#[test]
fn argparse_keyword_and_reorder() {
    assert_ok(&prog(
        "    argparse.add_option(name=\"--out\", default=\"x\")\n",
    ));
    assert_ok(&prog(
        "    argparse.add_option(default=\"x\", name=\"--out\")\n",
    ));
    assert_ok(&prog("    argparse.add_int(name=\"--n\", default=0)\n"));
    assert_ok(&prog("    argparse.set_description(description=\"d\")\n"));
    assert_ok(&prog("    auto v = argparse.get(name=\"--out\")\n"));
    assert_ok(&prog(
        "    auto xs = [\"a\"]\n    argparse.parse(argv=xs)\n",
    ));
}

#[test]
fn argparse_unknown_keyword_rejected() {
    assert_code(
        &prog("    argparse.add_option(flag=\"--out\", default=\"x\")\n"),
        "ZAP4023",
    );
}

#[test]
fn argparse_positional_unchanged() {
    assert_ok(&prog("    argparse.add_option(\"--out\", \"x\")\n"));
}

// ---------------------------------------------------------------------------
// xml — DOM parse / serialize / construct.
// ---------------------------------------------------------------------------

#[test]
fn xml_keyword_binding() {
    assert_ok(&prog("    auto e = xml.parse(s=\"<a/>\")\n"));
    assert_ok(&prog(
        "    auto e = xml.element(name=\"a\")\n    auto out = xml.dumps(e=e)\n",
    ));
}

#[test]
fn xml_unknown_keyword_rejected() {
    assert_code(&prog("    auto e = xml.parse(text=\"<a/>\")\n"), "ZAP4023");
}

#[test]
fn xml_positional_unchanged() {
    assert_ok(&prog("    auto e = xml.parse(\"<a/>\")\n"));
}

// ---------------------------------------------------------------------------
// concurrent — lock-free Counter; `start` optional.
// ---------------------------------------------------------------------------

#[test]
fn concurrent_counter_keyword_and_default() {
    assert_ok(&prog("    auto c = concurrent.Counter(start=5)\n"));
    assert_ok(&prog("    auto c = concurrent.Counter()\n"));
}

#[test]
fn concurrent_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto c = concurrent.Counter(initial=5)\n"),
        "ZAP4023",
    );
}

#[test]
fn concurrent_positional_unchanged() {
    assert_ok(&prog("    auto c = concurrent.Counter(5)\n"));
}

// ---------------------------------------------------------------------------
// bisect — binary search over a sorted list; Python names (a / x).
// ---------------------------------------------------------------------------

const SORTED: &str = "    auto xs = [1, 2, 3, 5]\n";

#[test]
fn bisect_keyword_and_reorder() {
    assert_ok(&prog(&format!(
        "{SORTED}    auto i = bisect.bisect_right(a=xs, x=4)\n"
    )));
    assert_ok(&prog(&format!(
        "{SORTED}    auto i = bisect.bisect_right(x=4, a=xs)\n"
    )));
    assert_ok(&prog(&format!(
        "{SORTED}    bisect.insort_left(a=xs, x=4)\n"
    )));
}

#[test]
fn bisect_unknown_keyword_rejected() {
    assert_code(
        &prog(&format!(
            "{SORTED}    auto i = bisect.bisect_right(lst=xs, x=4)\n"
        )),
        "ZAP4023",
    );
}

#[test]
fn bisect_positional_unchanged() {
    assert_ok(&prog(&format!(
        "{SORTED}    auto i = bisect.bisect_right(xs, 4)\n"
    )));
}

// ---------------------------------------------------------------------------
// url parse / build (#970, D-0209). `join` is the two-argument method that most
// benefits from keywords (`base` / `ref`); the rest are single-argument and
// rowed for a uniform keyword-capable surface.
// ---------------------------------------------------------------------------

#[test]
fn url_join_keyword_and_reorder() {
    assert_ok(&prog(
        "    auto u = url.join(base=\"http://a/b/c\", ref=\"../g\")\n",
    ));
    // Reordered by name — the two same-typed str args are the reason to allow
    // keywords here (guards against an accidental base/ref swap).
    assert_ok(&prog(
        "    auto u = url.join(ref=\"../g\", base=\"http://a/b/c\")\n",
    ));
}

#[test]
fn url_join_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto u = url.join(base=\"http://a/b/c\", url=\"../g\")\n"),
        "ZAP4023",
    );
}

#[test]
fn url_join_positional_unchanged() {
    assert_ok(&prog("    auto u = url.join(\"http://a/b/c\", \"../g\")\n"));
}

#[test]
fn url_split_keyword() {
    assert_ok(&prog("    auto p = url.split(s=\"http://h/path\")\n"));
}

#[test]
fn url_build_keyword() {
    assert_ok(&prog(
        "    auto p = url.split(\"http://h/path\")\n    auto s = url.build(parts=p)\n",
    ));
}

#[test]
fn url_parse_query_keyword() {
    assert_ok(&prog("    auto q = url.parse_query(q=\"a=1&b=2\")\n"));
    assert_ok(&prog("    auto q = url.parse_query_first(q=\"a=1&b=2\")\n"));
}

#[test]
fn url_encode_query_multi_keyword() {
    assert_ok(&prog(
        "    auto d = {\"a\": [\"1\", \"2\"]}\n    auto s = url.encode_query_multi(d=d)\n",
    ));
}

#[test]
fn url_parse_query_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto q = url.parse_query(query=\"a=1\")\n"),
        "ZAP4023",
    );
}

// ---------------------------------------------------------------------------
// os/fs filesystem gaps (#968, D-0211): the new multi-arg methods take keywords.
// ---------------------------------------------------------------------------

#[test]
fn fs_rename_keyword_and_reorder() {
    assert_ok(&prog("    fs.rename(src=\"a\", dst=\"b\")\n"));
    assert_ok(&prog("    fs.rename(dst=\"b\", src=\"a\")\n"));
}

#[test]
fn fs_rename_unknown_keyword_rejected() {
    assert_code(&prog("    fs.rename(src=\"a\", to=\"b\")\n"), "ZAP4023");
}

#[test]
fn os_path_relpath_keyword() {
    assert_ok(&prog(
        "    auto r = os.path_relpath(path=\"/a/b\", start=\"/a\")\n",
    ));
    assert_ok(&prog(
        "    auto r = os.path_relpath(start=\"/a\", path=\"/a/b\")\n",
    ));
}

#[test]
fn fs_stat_keyword_and_positional() {
    assert_ok(&prog("    auto s = fs.stat(path=\"/tmp\")\n"));
    assert_ok(&prog("    auto s = fs.stat(\"/tmp\")\n"));
}

#[test]
fn fs_copy_positional_unchanged() {
    assert_ok(&prog("    fs.copy(\"a\", \"b\")\n"));
}

// ---------------------------------------------------------------------------
// Line-oriented input (#969, D-0212): the buffered reader + fs.lines. `input`
// is a global builtin (not a seam function), so it has no seam_sigs row and no
// keyword surface — like `print`.
// ---------------------------------------------------------------------------

#[test]
fn io_reader_keyword_and_positional() {
    // Keyword binding for the reader verbs, plus readn keyword reordering.
    assert_ok(&prog(
        "    auto fd = io.open(\"/tmp/x\", \"r\")\n    \
         auto r = io.reader(fd=fd)\n    \
         auto l = io.readline(reader=r)\n    \
         auto a = io.readall(reader=r)\n    \
         auto c = io.readn(reader=r, n=8)\n",
    ));
    assert_ok(&prog(
        "    auto fd = io.open(\"/tmp/x\", \"r\")\n    \
         auto r = io.reader(fd=fd)\n    \
         auto c = io.readn(n=8, reader=r)\n",
    ));
    // Positional forms unchanged.
    assert_ok(&prog(
        "    auto fd = io.open(\"/tmp/x\", \"r\")\n    \
         auto r = io.reader(fd)\n    \
         auto l = io.readline(r)\n    \
         auto c = io.readn(r, 8)\n",
    ));
}

#[test]
fn io_reader_unknown_keyword_rejected() {
    assert_code(&prog("    auto r = io.reader(descriptor=0)\n"), "ZAP4023");
    assert_code(
        &prog(
            "    auto fd = io.open(\"/tmp/x\", \"r\")\n    \
             auto r = io.reader(fd)\n    \
             auto c = io.readn(reader=r, count=8)\n",
        ),
        "ZAP4023",
    );
}

#[test]
fn fs_lines_keyword_and_positional() {
    assert_ok(&prog("    auto ls = fs.lines(path=\"/tmp/x\")\n"));
    assert_ok(&prog("    auto ls = fs.lines(\"/tmp/x\")\n"));
    assert_code(
        &prog("    auto ls = fs.lines(file=\"/tmp/x\")\n"),
        "ZAP4023",
    );
}

#[test]
fn fs_glob_fnmatch_keyword_and_positional() {
    // Path matching (#1033, D-0216): keyword binding, reordering, positional.
    assert_ok(&prog(
        "    auto hit = fs.fnmatch(name=\"a.txt\", pattern=\"*.txt\")\n",
    ));
    assert_ok(&prog(
        "    auto hit = fs.fnmatch(pattern=\"*.txt\", name=\"a.txt\")\n",
    ));
    assert_ok(&prog("    auto hit = fs.fnmatch(\"a.txt\", \"*.txt\")\n"));
    assert_ok(&prog(
        "    auto ps = fs.glob(root=\"/tmp\", pattern=\"**/*.zap\")\n",
    ));
    assert_ok(&prog("    auto ps = fs.glob(\"/tmp\", \"*.zap\")\n"));
    assert_code(
        &prog("    auto hit = fs.fnmatch(name=\"a\", glob=\"*\")\n"),
        "ZAP4023",
    );
}
