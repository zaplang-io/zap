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
    // `bogus` is not a parameter of `http.get` (`timeout` now is — #986).
    assert_code(
        &prog("    auto r = http.get(\"http://x\", bogus=5)\n"),
        "ZAP4023",
    );
}

// ---------------------------------------------------------------------------
// Per-request `timeout` (#986): a trailing optional ms total-deadline on the
// client verbs. `headers` sits before it and is an INTERIOR optional — the
// binder fills it with `{}` when a caller supplies only `timeout`, so
// `http.get(url, timeout=5000)` binds without the empty-dict wart.
// ---------------------------------------------------------------------------

#[test]
fn http_get_timeout_only() {
    // Interior `headers` omitted, trailing `timeout` supplied by keyword.
    assert_ok(&prog("    auto r = http.get(\"http://x\", timeout=5000)\n"));
}

#[test]
fn http_get_headers_and_timeout() {
    assert_ok(&prog(
        "    auto h = {\"a\": \"b\"}\n    auto r = http.get(\"http://x\", headers=h, timeout=5000)\n",
    ));
}

#[test]
fn http_get_headers_positional_timeout_keyword() {
    assert_ok(&prog(
        "    auto h = {\"a\": \"b\"}\n    auto r = http.get(\"http://x\", h, timeout=5000)\n",
    ));
}

#[test]
fn http_post_timeout_only() {
    assert_ok(&prog(
        "    auto r = http.post(\"http://x\", \"n=zap\", timeout=2500)\n",
    ));
}

#[test]
fn http_post_headers_and_timeout_reordered() {
    assert_ok(&prog(
        "    auto h = {\"a\": \"b\"}\n    auto r = http.post(timeout=2500, url=\"http://x\", body=\"n=zap\", headers=h)\n",
    ));
}

#[test]
fn http_request_timeout_only() {
    assert_ok(&prog(
        "    auto r = http.request(\"GET\", \"http://x\", \"\", timeout=1000)\n",
    ));
}

#[test]
fn http_request_headers_and_timeout() {
    assert_ok(&prog(
        "    auto h = {\"a\": \"b\"}\n    auto r = http.request(\"GET\", \"http://x\", \"\", h, timeout=1000)\n",
    ));
}

#[test]
fn http_get_too_many_args_rejected() {
    // url + headers + timeout is the max (3); a 4th positional is an arity error.
    assert_code(
        &prog("    auto r = http.get(\"http://x\", {}, 5000, 1)\n"),
        "ZAP4025",
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

// redirect-following result accessors (#987): single-arg, keyword-capable.
#[test]
fn http_redirect_accessors_keyword_and_positional() {
    assert_ok(&prog(
        "    auto r = http.get(\"http://x\")\n    auto u = http.final_url(resp=r)\n    auto c = http.redirects(resp=r)\n",
    ));
    assert_ok(&prog(
        "    auto r = http.get(\"http://x\")\n    auto u = http.final_url(r)\n    auto c = http.redirects(r)\n",
    ));
}

#[test]
fn http_redirect_accessor_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto r = http.get(\"http://x\")\n    auto u = http.final_url(response=r)\n"),
        "ZAP4023",
    );
}

// Client response header enumeration + Set-Cookie parsing (#989).
#[test]
fn http_headers_and_parse_set_cookie_keywords() {
    assert_ok(&prog(
        "    auto r = http.get(\"http://x\")\n    auto hs = http.headers(resp=r)\n",
    ));
    assert_ok(&prog(
        "    auto c = http.parse_set_cookie(value=\"a=b; Path=/; Secure\")\n",
    ));
    assert_ok(&prog(
        "    auto c = http.parse_set_cookie(\"a=b\")\n    auto nm = c.name\n    auto sec = c.secure\n",
    ));
}

#[test]
fn http_parse_set_cookie_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto c = http.parse_set_cookie(cookie=\"a=b\")\n"),
        "ZAP4023",
    );
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

// --- #973 scalar additions: keyword binding across the new surface ----------

#[test]
fn math_new_scalar_keywords() {
    // Unary f64 -> f64 additions bind their `x`.
    assert_ok(&prog("    auto a = math.degrees(x=3.14)\n"));
    assert_ok(&prog("    auto b = math.radians(x=180.0)\n"));
    assert_ok(&prog("    auto c = math.sinh(x=1.0)\n"));
    assert_ok(&prog("    auto d = math.cbrt(x=27.0)\n"));
    assert_ok(&prog("    auto e = math.gamma(x=5.0)\n"));
    // Binary compose forms, including reordering.
    assert_ok(&prog("    auto f = math.copysign(x=1.0, y=-2.0)\n"));
    assert_ok(&prog("    auto g = math.copysign(y=-2.0, x=1.0)\n"));
    assert_ok(&prog("    auto h = math.ldexp(x=1.5, i=3)\n"));
}

#[test]
fn math_integer_math_keywords() {
    assert_ok(&prog("    auto a = math.gcd(a=12, b=8)\n"));
    assert_ok(&prog("    auto b = math.gcd(b=8, a=12)\n"));
    assert_ok(&prog("    auto c = math.comb(n=5, k=2)\n"));
    assert_ok(&prog("    auto d = math.perm(k=2, n=5)\n"));
    assert_ok(&prog("    auto e = math.factorial(n=6)\n"));
    assert_ok(&prog("    auto f = math.isqrt(n=17)\n"));
}

// isclose: the two tolerances are optional keyword args with defaults. All four
// supply patterns must bind, including the interior-hole case (abs_tol given,
// rel_tol defaulted via SeamDefault::Float).

#[test]
fn math_isclose_default_tolerances() {
    assert_ok(&prog("    auto a = math.isclose(1.0, 1.0)\n"));
}

#[test]
fn math_isclose_rel_tol_only() {
    assert_ok(&prog("    auto a = math.isclose(1.0, 1.1, rel_tol=0.2)\n"));
}

#[test]
fn math_isclose_abs_tol_only_fills_interior_rel_tol() {
    // `rel_tol` is an INTERIOR optional here — the binder fills it from its
    // SeamDefault::Float("1e-9") so the call binds (the #986 interior-hole path
    // generalized to a float default, D-0234).
    assert_ok(&prog("    auto a = math.isclose(1.0, 1.0, abs_tol=0.1)\n"));
}

#[test]
fn math_isclose_both_tolerances_reordered() {
    assert_ok(&prog(
        "    auto a = math.isclose(a=1.0, b=1.1, abs_tol=0.1, rel_tol=0.2)\n",
    ));
}

#[test]
fn math_isclose_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto a = math.isclose(1.0, 1.0, tol=0.1)\n"),
        "ZAP4023",
    );
}

#[test]
fn math_reduction_keywords() {
    assert_ok(&prog(
        "    auto xs = [1.0, 2.0, 3.0]\n    auto s = math.fsum(xs=xs)\n",
    ));
    assert_ok(&prog(
        "    auto p = [1.0, 2.0]\n    auto q = [3.0, 4.0]\n    auto d = math.dist(p=p, q=q)\n",
    ));
    assert_ok(&prog(
        "    auto p = [1.0, 2.0]\n    auto q = [3.0, 4.0]\n    auto s = math.sumprod(q=q, p=p)\n",
    ));
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
    // The two bounded dials take a `deadline_ns` keyword (#1158), reorderable.
    assert_ok(&prog(
        "    auto c = net.connect_deadline(host=\"localhost\", port=80, deadline_ns=0)\n",
    ));
    assert_ok(&prog(
        "    auto c = net.connect_deadline(deadline_ns=0, port=80, host=\"h\")\n",
    ));
    assert_ok(&prog(
        "    auto c = net.connect_ip_deadline(ip=\"127.0.0.1\", port=80, deadline_ns=0)\n",
    ));
    assert_code(
        &prog("    auto c = net.connect_deadline(host=\"h\", port=80, deadline=0)\n"),
        "ZAP4023",
    );
}

#[test]
fn math_net_positional_unchanged() {
    assert_ok(&prog("    auto p = math.pow(2.0, 10.0)\n"));
    assert_ok(&prog("    auto l = net.listen(\"0.0.0.0\", 8080)\n"));
    assert_ok(&prog("    auto c = net.connect_deadline(\"h\", 80, 0)\n"));
    assert_ok(&prog(
        "    auto c = net.connect_ip_deadline(\"127.0.0.1\", 80, 0)\n",
    ));
}

#[test]
fn net_udp_keyword_reorder_and_unknown() {
    // udp_bind(sock, host, port): name-order reorder and swapped host/port.
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.udp_bind(sock=s, host=\"127.0.0.1\", port=0)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.udp_bind(port=0, host=\"127.0.0.1\", sock=s)\n",
    ));
    // udp_connect mirrors udp_bind's (sock, host, port) shape.
    assert_ok(&prog(
        "    auto s = net.udp_socket6()\n    net.udp_connect(sock=s, host=\"::1\", port=9)\n",
    ));
    // sendto(sock, data, host, port) -> int64; reordered + the 4th slot named.
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    auto n = net.sendto(data=\"ping\", sock=s, port=9, host=\"127.0.0.1\")\n",
    ));
    // recvfrom(sock, n) -> (data, host, port): keyword form + tuple destructure.
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    auto d, h, p = net.recvfrom(n=1024, sock=s)\n",
    ));
    // An unknown keyword is rejected by the central binder (ZAP4023).
    assert_code(
        &prog(
            "    auto s = net.udp_socket()\n    net.udp_bind(sock=s, host=\"127.0.0.1\", prt=0)\n",
        ),
        "ZAP4023",
    );
}

#[test]
fn net_udp_positional_unchanged() {
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.udp_bind(s, \"127.0.0.1\", 0)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    auto n = net.sendto(s, \"x\", \"127.0.0.1\", 9)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    auto d, h, p = net.recvfrom(s, 64)\n",
    ));
}

#[test]
fn net_dns_keyword_and_positional() {
    // lookup_host/ipv4/ipv6(name) and lookup_addr(ip): keyword + positional.
    assert_ok(&prog(
        "    auto a = net.lookup_host(name=\"example.com\")\n",
    ));
    assert_ok(&prog("    auto a = net.lookup_host(\"example.com\")\n"));
    assert_ok(&prog(
        "    auto a = net.lookup_ipv4(name=\"example.com\")\n",
    ));
    assert_ok(&prog(
        "    auto a = net.lookup_ipv6(name=\"example.com\")\n",
    ));
    assert_ok(&prog("    auto a = net.lookup_addr(ip=\"127.0.0.1\")\n"));
    assert_ok(&prog("    auto a = net.lookup_addr(\"127.0.0.1\")\n"));
    // The result is a list[str] — index + iterate type-check.
    assert_ok(&prog(
        "    auto a = net.lookup_host(\"h\")\n    auto first = a[0]\n    print(len(a))\n",
    ));
    // An unknown keyword is rejected by the central binder (ZAP4023).
    assert_code(
        &prog("    auto a = net.lookup_host(host=\"example.com\")\n"),
        "ZAP4023",
    );
}

#[test]
fn net_unix_keyword_and_positional() {
    // Stream: listen_unix / connect_unix take a single `path`.
    assert_ok(&prog(
        "    auto l = net.listen_unix(path=\"/tmp/x.sock\")\n",
    ));
    assert_ok(&prog("    auto c = net.connect_unix(\"/tmp/x.sock\")\n"));
    // Datagram: unixgram_bind / sendto_unix / recvfrom_unix, keyword-reordered.
    assert_ok(&prog(
        "    auto s = net.unixgram_socket()\n    net.unixgram_bind(path=\"/tmp/d.sock\", sock=s)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.unixgram_socket()\n    auto n = net.sendto_unix(data=\"x\", sock=s, path=\"/tmp/d.sock\")\n",
    ));
    assert_ok(&prog(
        "    auto s = net.unixgram_socket()\n    auto d, p = net.recvfrom_unix(sock=s, n=64)\n",
    ));
    // recvfrom_unix yields a (str, str) tuple — positional destructure too.
    assert_ok(&prog(
        "    auto s = net.unixgram_socket()\n    auto d, p = net.recvfrom_unix(s, 64)\n",
    ));
    // An unknown keyword is rejected by the central binder (ZAP4023).
    assert_code(
        &prog("    auto l = net.listen_unix(file=\"/tmp/x.sock\")\n"),
        "ZAP4023",
    );
    // peer_cred(sock) -> (uid, gid, pid): keyword + positional destructure.
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    auto u, g, p = net.peer_cred(sock=s)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    auto u, g, p = net.peer_cred(s)\n",
    ));
}

#[test]
fn net_sockopt_keyword_and_positional() {
    // Setters take (sock, on|nbytes), reorderable by name.
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.set_reuseaddr(on=True, sock=s)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.set_nodelay(s, True)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.set_recv_buffer(nbytes=65536, sock=s)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.set_send_buffer(s, 65536)\n",
    ));
    // Getters: bool getters feed a condition; int getters feed arithmetic.
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    if net.get_nodelay(sock=s):\n        print(1)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    auto n = net.get_recv_buffer(s) + 1\n",
    ));
    // An unknown keyword is rejected by the central binder (ZAP4023).
    assert_code(
        &prog("    auto s = net.udp_socket()\n    net.set_nodelay(sock=s, enable=True)\n"),
        "ZAP4023",
    );
}

#[test]
fn net_keepalive_interval_keyword_and_positional() {
    // Setters take (sock, seconds|count), reorderable by name.
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.set_keepalive_idle(seconds=60, sock=s)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.set_keepalive_interval(s, 10)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.set_keepalive_count(count=5, sock=s)\n",
    ));
    // Getters -> int64 feed arithmetic.
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    auto n = net.get_keepalive_idle(s) + net.get_keepalive_count(sock=s)\n",
    ));
    // An unknown keyword is rejected by the central binder (ZAP4023).
    assert_code(
        &prog("    auto s = net.udp_socket()\n    net.set_keepalive_idle(sock=s, secs=60)\n"),
        "ZAP4023",
    );
}

#[test]
fn net_multicast_keyword_and_positional() {
    // join/leave take (sock, group); reorderable by name.
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.join_multicast(group=\"239.1.2.3\", sock=s)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.leave_multicast(s, \"239.1.2.3\")\n",
    ));
    // sender options: ttl (hops) and loopback (bool).
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.set_multicast_ttl(sock=s, hops=1)\n",
    ));
    assert_ok(&prog(
        "    auto s = net.udp_socket()\n    net.set_multicast_loopback(s, True)\n",
    ));
    // An unknown keyword is rejected by the central binder (ZAP4023).
    assert_code(
        &prog(
            "    auto s = net.udp_socket()\n    net.join_multicast(sock=s, addr=\"239.1.2.3\")\n",
        ),
        "ZAP4023",
    );
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

// The object-model constructor `argparse.ArgumentParser(prog="",
// description="")` (#997, D-0238): both arguments optional, keyword-bindable,
// reorderable, and defaultable through the seam.
#[test]
fn argparse_parser_ctor_keyword_and_reorder() {
    // Zero args: both default.
    assert_ok(&prog("    auto p = argparse.ArgumentParser()\n"));
    // One positional.
    assert_ok(&prog("    auto p = argparse.ArgumentParser(\"tool\")\n"));
    // Both positional.
    assert_ok(&prog(
        "    auto p = argparse.ArgumentParser(\"tool\", \"desc\")\n",
    ));
    // Both by keyword.
    assert_ok(&prog(
        "    auto p = argparse.ArgumentParser(prog=\"tool\", description=\"desc\")\n",
    ));
    // Reordered keywords.
    assert_ok(&prog(
        "    auto p = argparse.ArgumentParser(description=\"desc\", prog=\"tool\")\n",
    ));
    // A keyword with a leading hole: `prog` must default (interior fill).
    assert_ok(&prog(
        "    auto p = argparse.ArgumentParser(description=\"desc\")\n",
    ));
    // Only the leading keyword: `description` stays absent (trailing default).
    assert_ok(&prog(
        "    auto p = argparse.ArgumentParser(prog=\"tool\")\n",
    ));
}

#[test]
fn argparse_parser_ctor_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto p = argparse.ArgumentParser(name=\"tool\")\n"),
        "ZAP4023",
    );
}

#[test]
fn argparse_parser_ctor_too_many_args_rejected() {
    assert_code(
        &prog("    auto p = argparse.ArgumentParser(\"a\", \"b\", \"c\")\n"),
        "ZAP4025",
    );
}

#[test]
fn argparse_parser_ctor_bad_arg_type_rejected() {
    assert_code(
        &prog("    auto p = argparse.ArgumentParser(prog=5)\n"),
        "ZAP4024",
    );
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
// heap — nlargest / nsmallest (#978, D-0236). Python names `n` / `iterable` /
// `key`; `key` is the trailing optional selector.
// ---------------------------------------------------------------------------

const HEAP_XS: &str = "    auto xs = [5, 1, 3, 2, 4]\n";

#[test]
fn heap_topk_keyword_and_reorder() {
    // Bind by name, in declared order and reversed.
    assert_ok(&prog(&format!(
        "{HEAP_XS}    auto top = heap.nlargest(n=2, iterable=xs)\n"
    )));
    assert_ok(&prog(&format!(
        "{HEAP_XS}    auto bot = heap.nsmallest(iterable=xs, n=2)\n"
    )));
}

#[test]
fn heap_topk_key_keyword() {
    // The `key=` selector binds by name alongside positional `n`/`iterable`.
    let src = format!(
        "def negate(int64 x) -> int64:\n    return -x\n\ndef f():\n{HEAP_XS}    auto top = heap.nlargest(2, xs, key=negate)\n"
    );
    assert_ok(&src);
}

#[test]
fn heap_topk_unknown_keyword_rejected() {
    // A misspelled keyword is the same ZAP4023 a user function emits.
    assert_code(
        &prog(&format!(
            "{HEAP_XS}    auto top = heap.nlargest(count=2, iterable=xs)\n"
        )),
        "ZAP4023",
    );
}

#[test]
fn heap_topk_positional_unchanged() {
    assert_ok(&prog(&format!(
        "{HEAP_XS}    auto top = heap.nlargest(2, xs)\n"
    )));
    assert_ok(&prog(&format!(
        "{HEAP_XS}    auto bot = heap.nsmallest(3, xs)\n"
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
fn os_path_split_splitext_keyword_and_positional() {
    // Tuple-returning decomposition (#1030, D-0219).
    assert_ok(&prog("    auto h, t = os.path_split(path=\"/a/b.txt\")\n"));
    assert_ok(&prog("    auto h, t = os.path_split(\"/a/b.txt\")\n"));
    assert_ok(&prog(
        "    auto r, e = os.path_splitext(path=\"/a/b.txt\")\n",
    ));
    assert_code(
        &prog("    auto h, t = os.path_split(p=\"/a\")\n"),
        "ZAP4023",
    );
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

// The `for line in file` iterator wrappers (#1031, D-0223).
#[test]
fn io_lines_and_fs_open_reader_keyword_and_positional() {
    assert_ok(&prog(
        "    auto fd = io.open(\"/tmp/x\", \"r\")\n    \
         auto r = io.reader(fd)\n    \
         auto it = io.lines(reader=r)\n",
    ));
    assert_ok(&prog(
        "    auto fd = io.open(\"/tmp/x\", \"r\")\n    \
         auto r = io.reader(fd)\n    \
         auto it = io.lines(r)\n",
    ));
    assert_ok(&prog("    auto it = fs.open_reader(path=\"/tmp/x\")\n"));
    assert_ok(&prog("    auto it = fs.open_reader(\"/tmp/x\")\n"));
}

#[test]
fn io_lines_and_fs_open_reader_unknown_keyword_rejected() {
    assert_code(
        &prog(
            "    auto fd = io.open(\"/tmp/x\", \"r\")\n    \
             auto r = io.reader(fd)\n    \
             auto it = io.lines(stream=r)\n",
        ),
        "ZAP4023",
    );
    assert_code(
        &prog("    auto it = fs.open_reader(file=\"/tmp/x\")\n"),
        "ZAP4023",
    );
}

// Buffered writer / file object (#1071, D-0240): io.writer(fd) + the prelude
// `Writer` methods. `io.writer` is the only public seam fn (one row); the
// `_writer_*` delegates are internal and positional (no rows).
#[test]
fn io_writer_keyword_and_positional() {
    assert_ok(&prog(
        "    auto fd = io.open(\"/tmp/x\", \"w\")\n    \
         auto w = io.writer(fd=fd)\n",
    ));
    assert_ok(&prog(
        "    auto fd = io.open(\"/tmp/x\", \"w\")\n    \
         auto w = io.writer(fd)\n",
    ));
}

#[test]
fn io_writer_unknown_keyword_rejected() {
    assert_code(&prog("    auto w = io.writer(descriptor=0)\n"), "ZAP4023");
}

// The `Writer` object surface and its context-manager form type-check: `write`
// returns int64, `flush`/`close_` return unit, and `with io.writer(fd) as w:`
// binds `w` to a Writer (D-0069 __enter__). Shape coverage for #1071.
#[test]
fn io_writer_object_methods_and_with_form() {
    assert_ok(&prog(
        "    auto fd = io.open(\"/tmp/x\", \"w\")\n    \
         auto w = io.writer(fd)\n    \
         auto n = w.write(\"hello\\n\")\n    \
         w.flush()\n    \
         w.close_()\n",
    ));
    assert_ok(&prog(
        "    auto fd = io.open(\"/tmp/x\", \"w\")\n    \
         with io.writer(fd) as w:\n        \
             auto n = w.write(\"line\\n\")\n",
    ));
}

// Directory traversal (#1032, D-0224): scandir / walk / copytree.
#[test]
fn fs_traversal_keyword_and_positional() {
    assert_ok(&prog("    auto es = fs.scandir(path=\"/tmp/x\")\n"));
    assert_ok(&prog("    auto es = fs.scandir(\"/tmp/x\")\n"));
    assert_ok(&prog("    auto w = fs.walk(root=\"/tmp/x\")\n"));
    assert_ok(&prog("    fs.copytree(src=\"/tmp/a\", dst=\"/tmp/b\")\n"));
    assert_ok(&prog("    fs.copytree(dst=\"/tmp/b\", src=\"/tmp/a\")\n"));
}

#[test]
fn fs_traversal_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto es = fs.scandir(dir=\"/tmp/x\")\n"),
        "ZAP4023",
    );
    assert_code(
        &prog("    fs.copytree(from=\"/tmp/a\", to=\"/tmp/b\")\n"),
        "ZAP4023",
    );
}

#[test]
fn sys_write_keyword_and_positional() {
    // Standard-stream write (#1035, D-0217). stdin/stdout/stderr take no args.
    assert_ok(&prog(
        "    auto out = sys.stdout()\n    auto n = sys.write(stream=out, data=\"hi\")\n",
    ));
    assert_ok(&prog(
        "    auto out = sys.stdout()\n    auto n = sys.write(data=\"hi\", stream=out)\n",
    ));
    assert_ok(&prog(
        "    auto out = sys.stdout()\n    auto n = sys.write(out, \"hi\")\n",
    ));
    assert_code(
        &prog("    auto out = sys.stdout()\n    auto n = sys.write(stream=out, payload=\"hi\")\n"),
        "ZAP4023",
    );
}

#[test]
fn datetime_arith_keyword_and_positional() {
    // Arithmetic / comparison / field helpers (#974, D-0221; add takes a
    // Duration since #1047, D-0227).
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto one = time.seconds(1)\n    auto b = datetime.add(dt=t, delta=one)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto d = datetime.diff(a=t, b=t)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto w = datetime.weekday(dt=t)\n",
    ));
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto one = time.seconds(1)\n    auto b = datetime.add(dt=t, secs=one)\n"),
        "ZAP4023",
    );
}

#[test]
fn datetime_timezone_keyword_and_positional() {
    // Timezone surface (#1045, D-0225): keyword binding + reordering.
    assert_ok(&prog("    auto z = datetime.zone(name=\"UTC\")\n"));
    assert_ok(&prog("    auto z = datetime.fixed_offset(minutes=330)\n"));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto s = datetime.isoformat(dt=t)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto s = datetime.rfc3339(dt=t)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.rfc3339_parse(s=\"2021-01-01T00:00:00Z\")\n",
    ));
    // astimezone — bind the DateTime/TimeZone to vars first (a nested call as a
    // kwarg value does not parse), then reorder the two keywords.
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto z = datetime.fixed_offset(330)\n    auto a = datetime.astimezone(dt=t, tz=z)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto z = datetime.fixed_offset(330)\n    auto a = datetime.astimezone(tz=z, dt=t)\n",
    ));
    // Positional unchanged.
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto z = datetime.fixed_offset(330)\n    auto a = datetime.astimezone(t, z)\n",
    ));
    // A misspelled keyword is rejected (no longer silently ignored).
    assert_code(&prog("    auto z = datetime.zone(nm=\"UTC\")\n"), "ZAP4023");
}

#[test]
fn datetime_date_time_types_keyword_and_positional() {
    // date/time constructors (#1046, D-0226): keyword binding + reorder.
    assert_ok(&prog("    auto d = datetime.date(2020, 2, 29)\n"));
    assert_ok(&prog(
        "    auto d = datetime.date(day=1, year=2020, month=6)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.time(hour=1, minute=2, second=3)\n",
    ));
    assert_ok(&prog(
        "    auto dt = datetime.from_timestamp(0)\n    auto d = datetime.to_date(dt=dt)\n",
    ));
    assert_ok(&prog(
        "    auto dt = datetime.from_timestamp(0)\n    auto t = datetime.to_time(dt=dt)\n",
    ));
    // combine — bind the Date/Time to vars first, then reorder the keywords.
    assert_ok(&prog(
        "    auto d = datetime.date(2020, 1, 1)\n    auto t = datetime.time(1, 2, 3)\n    auto dt = datetime.combine(time=t, date=d)\n",
    ));
    // Unknown keyword rejected.
    assert_code(
        &prog("    auto d = datetime.date(yr=2020, month=1, day=1)\n"),
        "ZAP4023",
    );
}

#[test]
fn datetime_zone_resolution_keyword_and_positional() {
    // Named-zone civil resolution (#1149).
    // resolve — the TimeZone is positional first, then the six civil ints, with
    // an optional trailing `fold` (keyword or positional). Bespoke binding, so
    // it is NOT in the seam signatures (like `replace`).
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    auto d = datetime.resolve(z, 2024, 3, 10, 2, 30, 0)\n",
    ));
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    auto d = datetime.resolve(z, 2024, 11, 3, 1, 30, 0, fold=1)\n",
    ));
    // The trailing fold may also be positional.
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    auto d = datetime.resolve(z, 2024, 11, 3, 1, 30, 0, 1)\n",
    ));
    // The TimeZone must be positional (a `tz=` keyword in slot 0 is rejected).
    assert_code(
        &prog("    auto z = datetime.zone(\"UTC\")\n    auto d = datetime.resolve(tz=z, 2024, 1, 1, 0, 0, 0)\n"),
        "ZAP4023",
    );
    // A stray keyword other than the trailing `fold` is rejected.
    assert_code(
        &prog("    auto z = datetime.zone(\"UTC\")\n    auto d = datetime.resolve(z, 2024, 1, 1, 0, 0, 0, fld=1)\n"),
        "ZAP4023",
    );
    // Wrong arity is rejected.
    assert_code(
        &prog(
            "    auto z = datetime.zone(\"UTC\")\n    auto d = datetime.resolve(z, 2024, 1, 1)\n",
        ),
        "ZAP4025",
    );

    // is_ambiguous / is_imaginary / resolve_imaginary — a single DateTime, so
    // they bind the `dt=` keyword like the other DateTime-consuming facets.
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    auto d = datetime.resolve(z, 2024, 11, 3, 1, 30, 0)\n    auto b = datetime.is_ambiguous(dt=d)\n",
    ));
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    auto d = datetime.resolve(z, 2024, 3, 10, 2, 30, 0)\n    auto b = datetime.is_imaginary(dt=d)\n",
    ));
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    auto d = datetime.resolve(z, 2024, 3, 10, 2, 30, 0)\n    auto r = datetime.resolve_imaginary(dt=d)\n",
    ));
    // Positional unchanged.
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    auto d = datetime.resolve(z, 2024, 3, 10, 2, 30, 0)\n    auto b = datetime.is_imaginary(d)\n",
    ));
    // A misspelled keyword is rejected.
    assert_code(
        &prog("    auto z = datetime.zone(\"UTC\")\n    auto d = datetime.resolve(z, 2024, 1, 1, 0, 0, 0)\n    auto b = datetime.is_ambiguous(x=d)\n"),
        "ZAP4023",
    );
}

#[test]
fn datetime_subsecond_keyword_and_positional() {
    // #1148 — replace()'s new fractional keywords. Bespoke binder (not a seam
    // row, like the civil fields): microsecond= / nanosecond= are optional,
    // any-subset, any-order keywords alongside the civil fields.
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, microsecond=5)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, nanosecond=5)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, hour=1, microsecond=250000)\n",
    ));
    // microsecond= and nanosecond= both set the one stored fraction -> at most one.
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, microsecond=1, nanosecond=2)\n"),
        "ZAP4023",
    );

    // #1171 — local_zone() binds as a 0-arg call (no seam row, like now()).
    assert_ok(&prog("    auto z = datetime.local_zone()\n"));
    assert_code(
        &prog("    auto z = datetime.local_zone(\"UTC\")\n"),
        "ZAP4025",
    );
}

#[test]
fn datetime_time_refinements_keyword_and_positional() {
    // #1055 refinements: keyword binding for the new 1-arg helpers.
    assert_ok(&prog(
        "    auto d = time.seconds(1)\n    auto s = time.format(d=d)\n",
    ));
    assert_ok(&prog(
        "    auto st = time.monotonic_ns()\n    auto e = time.since(start=st)\n",
    ));
    assert_ok(&prog(
        "    auto dd = datetime.date(2021, 1, 1)\n    auto w = datetime.date_weekday(d=dd)\n",
    ));
    assert_code(
        &prog("    auto dd = datetime.date(2021, 1, 1)\n    auto w = datetime.date_weekday(date=dd)\n"),
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

#[test]
fn uuid_namespaced_keyword_and_positional() {
    // uuid3/uuid5 (#975, D-0232): keyword binding, reordering, positional.
    assert_ok(&prog(
        "    auto u = uuid.uuid5(ns=uuid.namespace_dns(), name=\"example.com\")\n",
    ));
    assert_ok(&prog(
        "    auto u = uuid.uuid5(name=\"example.com\", ns=uuid.namespace_dns())\n",
    ));
    assert_ok(&prog(
        "    auto u = uuid.uuid3(uuid.namespace_url(), \"https://example.com\")\n",
    ));
    // Unknown keyword → ZAP4023 (the same a user function emits).
    assert_code(
        &prog("    auto u = uuid.uuid5(namespace=uuid.namespace_dns(), name=\"x\")\n"),
        "ZAP4023",
    );
}

#[test]
fn uuid_parse_keyword_and_positional() {
    assert_ok(&prog(
        "    auto u = uuid.parse(s=\"6ba7b810-9dad-11d1-80b4-00c04fd430c8\")\n",
    ));
    assert_ok(&prog(
        "    auto u = uuid.parse(\"6ba7b810-9dad-11d1-80b4-00c04fd430c8\")\n",
    ));
    assert_code(&prog("    auto u = uuid.parse(value=\"x\")\n"), "ZAP4023");
}

// ---------------------------------------------------------------------------
// `process.run` / `check_run` / `run_shell` option keywords (#976, D-0237).
// `cmd` is positional; `env`/`cwd`/`timeout`/`input`/`capture_output`/
// `merge_stderr` are keyword-bound, with interior holes filled from the
// SEAM_METHOD_DEFAULTS rows. `run_shell` takes a `str` command.
// ---------------------------------------------------------------------------

#[test]
fn process_run_bare_positional_unchanged() {
    assert_ok(&prog(
        "    auto c, o, e = process.run([\"/bin/echo\", \"hi\"])\n",
    ));
}

#[test]
fn process_run_timeout_only_fills_interior_holes() {
    // `env`/`cwd` sit before `timeout`; the binder must default them (no hole
    // error) when only `timeout` is supplied.
    assert_ok(&prog(
        "    auto c, o, e = process.run([\"/bin/sleep\", \"1\"], timeout=0.5)\n",
    ));
}

#[test]
fn process_run_input_keyword() {
    assert_ok(&prog(
        "    auto c, o, e = process.run([\"/bin/cat\"], input=\"hello\")\n",
    ));
}

#[test]
fn process_run_env_keyword() {
    assert_ok(&prog(
        "    auto m = {\"K\": \"V\"}\n    auto c, o, e = process.run([\"/bin/env\"], env=m)\n",
    ));
}

#[test]
fn process_run_cwd_keyword() {
    assert_ok(&prog(
        "    auto c, o, e = process.run([\"/bin/pwd\"], cwd=\"/tmp\")\n",
    ));
}

#[test]
fn process_run_capture_and_merge_keywords() {
    assert_ok(&prog(
        "    auto c, o, e = process.run([\"/bin/echo\", \"x\"], capture_output=True, merge_stderr=True)\n",
    ));
}

#[test]
fn process_run_all_options_reordered() {
    assert_ok(&prog(
        "    auto m = {\"K\": \"V\"}\n    auto c, o, e = process.run(merge_stderr=False, cmd=[\"/bin/env\"], env=m, cwd=\"/tmp\", timeout=1.0, input=\"\", capture_output=True)\n",
    ));
}

#[test]
fn process_run_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto c, o, e = process.run([\"/bin/echo\"], bogus=1)\n"),
        "ZAP4023",
    );
}

#[test]
fn process_run_duplicate_value_rejected() {
    // `cmd` supplied both positionally and by keyword → multiple values.
    assert_code(
        &prog("    auto c, o, e = process.run([\"/bin/echo\"], cmd=[\"/bin/x\"])\n"),
        "ZAP4023",
    );
}

#[test]
fn process_check_run_keywords() {
    assert_ok(&prog(
        "    auto c, o, e = process.check_run([\"/bin/false\"], timeout=1.0)\n",
    ));
}

#[test]
fn process_check_run_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto c, o, e = process.check_run([\"/bin/echo\"], check=True)\n"),
        "ZAP4023",
    );
}

#[test]
fn process_run_shell_str_command_and_options() {
    assert_ok(&prog(
        "    auto c, o, e = process.run_shell(\"echo hi\", merge_stderr=True)\n",
    ));
}

#[test]
fn process_run_shell_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto c, o, e = process.run_shell(\"echo\", shell=True)\n"),
        "ZAP4023",
    );
}

#[test]
fn process_popen_ctor_and_methods_typecheck() {
    // `process.Popen(...)` desugars to the prelude factory; its methods drive the
    // internal `_popen_*` primitives. This exercises the whole streaming surface
    // through sema (ctor sugar + struct methods + keyword options).
    assert_ok(&prog(
        "    auto p = process.Popen([\"/bin/cat\"])\n    auto n = p.write(\"hi\")\n    p.close_stdin()\n    auto out = p.read_stdout_all()\n    auto rc = p.wait()\n",
    ));
    assert_ok(&prog(
        "    auto p = process.Popen([\"/bin/cat\"], merge_stderr=True, stderr=False)\n    auto d = p.done()\n    auto code = p.poll()\n    p.terminate()\n    p.kill()\n    auto pid = p.pid()\n",
    ));
}

// ---------------------------------------------------------------------------
// fs tempfile (#979, D-0239): mkstemp / gettempdir / write_atomic keyword binding
// ---------------------------------------------------------------------------

#[test]
fn fs_mkstemp_no_args() {
    // All three parameters optional — the bare call returns the (fd, path) tuple.
    assert_ok(&prog("    auto fd, p = fs.mkstemp()\n"));
}

#[test]
fn fs_mkstemp_positional_prefix() {
    assert_ok(&prog("    auto fd, p = fs.mkstemp(\"build-\")\n"));
}

#[test]
fn fs_mkstemp_positional_prefix_suffix() {
    assert_ok(&prog("    auto fd, p = fs.mkstemp(\"build-\", \".tmp\")\n"));
}

#[test]
fn fs_mkstemp_positional_all_three() {
    assert_ok(&prog(
        "    auto fd, p = fs.mkstemp(\"build-\", \".tmp\", \"/tmp\")\n",
    ));
}

#[test]
fn fs_mkstemp_keyword_prefix_only() {
    assert_ok(&prog("    auto fd, p = fs.mkstemp(prefix=\"build-\")\n"));
}

#[test]
fn fs_mkstemp_keyword_suffix_only_fills_prefix_hole() {
    // `prefix` sits before `suffix`; the binder must default it (no hole error)
    // when only `suffix` is supplied by keyword.
    assert_ok(&prog("    auto fd, p = fs.mkstemp(suffix=\".log\")\n"));
}

#[test]
fn fs_mkstemp_keyword_dir_only_fills_prefix_and_suffix_holes() {
    // `prefix` and `suffix` both precede `dir`; both must default when only `dir`
    // is supplied.
    assert_ok(&prog("    auto fd, p = fs.mkstemp(dir=\"/var/tmp\")\n"));
}

#[test]
fn fs_mkstemp_all_keywords_reordered() {
    // Reordered keywords must still land in the right slots.
    assert_ok(&prog(
        "    auto fd, p = fs.mkstemp(dir=\"/tmp\", suffix=\".tmp\", prefix=\"build-\")\n",
    ));
}

#[test]
fn fs_mkstemp_positional_and_keyword_mix() {
    assert_ok(&prog(
        "    auto fd, p = fs.mkstemp(\"build-\", dir=\"/tmp\")\n",
    ));
}

#[test]
fn fs_mkstemp_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto fd, p = fs.mkstemp(prefx=\"typo\")\n"),
        "ZAP4023",
    );
}

#[test]
fn fs_mkstemp_duplicate_value_rejected() {
    // `prefix` supplied both positionally and by keyword.
    assert_code(
        &prog("    auto fd, p = fs.mkstemp(\"a\", prefix=\"b\")\n"),
        "ZAP4023",
    );
}

#[test]
fn fs_mkstemp_too_many_args_rejected() {
    assert_code(
        &prog("    auto fd, p = fs.mkstemp(\"a\", \"b\", \"c\", \"d\")\n"),
        "ZAP4025",
    );
}

#[test]
fn fs_gettempdir_ok() {
    assert_ok(&prog("    auto d = fs.gettempdir()\n"));
}

#[test]
fn fs_gettempdir_rejects_args() {
    assert_code(&prog("    auto d = fs.gettempdir(\"x\")\n"), "ZAP4025");
}

#[test]
fn fs_write_atomic_positional() {
    assert_ok(&prog("    fs.write_atomic(\"out.txt\", \"hello\")\n"));
}

#[test]
fn fs_write_atomic_keywords_reordered() {
    assert_ok(&prog(
        "    fs.write_atomic(data=\"hello\", path=\"out.txt\")\n",
    ));
}

#[test]
fn fs_write_atomic_unknown_keyword_rejected() {
    assert_code(
        &prog("    fs.write_atomic(\"out.txt\", contents=\"hello\")\n"),
        "ZAP4023",
    );
}

#[test]
fn fs_write_atomic_wrong_arity_rejected() {
    assert_code(&prog("    fs.write_atomic(\"out.txt\")\n"), "ZAP4025");
}

// temp_file (#1077): four optional args (prefix, suffix, dir, delete) — three
// optional str kwargs plus a bool `delete`. Mirrors the mkstemp binding matrix,
// and adds the bool `delete` slot (interior-hole fill when a later keyword is
// supplied alone) and the prelude `TempFile` method / `with` surface.

#[test]
fn fs_temp_file_no_args() {
    // All four parameters optional — the bare call returns a `TempFile`.
    assert_ok(&prog("    auto t = fs.temp_file()\n"));
}

#[test]
fn fs_temp_file_positional_prefix() {
    assert_ok(&prog("    auto t = fs.temp_file(\"build-\")\n"));
}

#[test]
fn fs_temp_file_positional_prefix_suffix() {
    assert_ok(&prog("    auto t = fs.temp_file(\"build-\", \".tmp\")\n"));
}

#[test]
fn fs_temp_file_positional_three_strings() {
    assert_ok(&prog(
        "    auto t = fs.temp_file(\"build-\", \".tmp\", \"/tmp\")\n",
    ));
}

#[test]
fn fs_temp_file_positional_all_four() {
    // The fourth positional is the bool `delete`.
    assert_ok(&prog(
        "    auto t = fs.temp_file(\"build-\", \".tmp\", \"/tmp\", False)\n",
    ));
}

#[test]
fn fs_temp_file_keyword_prefix_only() {
    assert_ok(&prog("    auto t = fs.temp_file(prefix=\"build-\")\n"));
}

#[test]
fn fs_temp_file_keyword_delete_only_fills_holes() {
    // `delete` is the last parameter; supplying it alone by keyword must default
    // prefix/suffix/dir (fill all three interior holes) with no hole error.
    assert_ok(&prog("    auto t = fs.temp_file(delete=False)\n"));
}

#[test]
fn fs_temp_file_keyword_dir_only_fills_prefix_and_suffix_holes() {
    assert_ok(&prog("    auto t = fs.temp_file(dir=\"/var/tmp\")\n"));
}

#[test]
fn fs_temp_file_all_keywords_reordered() {
    // Reordered keywords (including the trailing bool) must land in the right slots.
    assert_ok(&prog(
        "    auto t = fs.temp_file(delete=False, dir=\"/tmp\", suffix=\".tmp\", prefix=\"build-\")\n",
    ));
}

#[test]
fn fs_temp_file_positional_and_keyword_mix() {
    assert_ok(&prog(
        "    auto t = fs.temp_file(\"build-\", delete=False)\n",
    ));
}

#[test]
fn fs_temp_file_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto t = fs.temp_file(prefx=\"typo\")\n"),
        "ZAP4023",
    );
}

#[test]
fn fs_temp_file_duplicate_value_rejected() {
    // `prefix` supplied both positionally and by keyword.
    assert_code(
        &prog("    auto t = fs.temp_file(\"a\", prefix=\"b\")\n"),
        "ZAP4023",
    );
}

#[test]
fn fs_temp_file_too_many_args_rejected() {
    assert_code(
        &prog("    auto t = fs.temp_file(\"a\", \"b\", \"c\", True, \"e\")\n"),
        "ZAP4025",
    );
}

#[test]
fn fs_temp_file_methods_and_with_typecheck() {
    // The prelude `TempFile` surface: `write` returns int64, `name` is a public
    // str field, `flush`/`close_` return unit, and the `with` form drives
    // __enter__/__exit__. Exercising all of it keeps the prelude struct's types
    // pinned (sema only; the golden covers runtime behaviour).
    assert_ok(
        "def f():\n\
         \x20   with fs.temp_file(prefix=\"log-\", suffix=\".txt\") as t:\n\
         \x20       auto wrote = t.write(\"hello\")\n\
         \x20       t.flush()\n\
         \x20       auto name = t.name\n\
         \x20   auto t2 = fs.temp_file(delete=False)\n\
         \x20   t2.write(\"x\")\n\
         \x20   t2.close_()\n",
    );
}

// ---------------------------------------------------------------------------
// statistics (#980, D-0243): keyword binding, defaults, and arg-type checks.
// ---------------------------------------------------------------------------

#[test]
fn statistics_list_keyword_binds() {
    // The single-list functions name their list `xs`.
    assert_ok(&prog("    auto a = statistics.mean(xs=[1.0, 2.0, 3.0])\n"));
    assert_ok(&prog(
        "    auto b = statistics.median(xs=[1.0, 2.0, 3.0])\n",
    ));
    assert_ok(&prog("    auto c = statistics.mode(xs=[1.0, 1.0, 2.0])\n"));
    assert_ok(&prog("    auto d = statistics.multimode(xs=[1.0, 2.0])\n"));
}

#[test]
fn statistics_int_lists_accepted() {
    // An int list is accepted alongside a float list (D-0243).
    assert_ok(&prog("    auto a = statistics.mean(xs=[1, 2, 3])\n"));
    assert_ok(&prog("    auto b = statistics.pstdev([1, 2, 3, 4])\n"));
}

#[test]
fn statistics_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto a = statistics.mean(data=[1.0, 2.0])\n"),
        "ZAP4023",
    );
}

#[test]
fn statistics_non_numeric_list_rejected() {
    // A list of str is not a numeric sequence.
    assert_code(
        &prog("    auto a = statistics.mean([\"a\", \"b\"])\n"),
        "ZAP4024",
    );
}

#[test]
fn statistics_spread_optional_centre_keyword() {
    // `variance`/`stdev` bind their optional `xbar`; `pvariance`/`pstdev` bind `mu`.
    assert_ok(&prog("    auto a = statistics.variance([1.0, 2.0, 3.0])\n"));
    assert_ok(&prog(
        "    auto b = statistics.variance([1.0, 2.0, 3.0], xbar=2.0)\n",
    ));
    assert_ok(&prog(
        "    auto c = statistics.stdev([1.0, 2.0, 3.0], xbar=2.0)\n",
    ));
    assert_ok(&prog(
        "    auto d = statistics.pvariance([1.0, 2.0], mu=1.5)\n",
    ));
    assert_ok(&prog(
        "    auto e = statistics.pstdev([1.0, 2.0], mu=1.5)\n",
    ));
}

#[test]
fn statistics_variance_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto a = statistics.variance([1.0, 2.0], centre=1.0)\n"),
        "ZAP4023",
    );
}

#[test]
fn statistics_quantiles_keywords_and_defaults() {
    // `quantiles(xs, n=4, method='exclusive')`: all three bindings, plus the
    // interior `n` default when only `method` is given by keyword (D-0243).
    assert_ok(&prog(
        "    auto a = statistics.quantiles([1.0, 2.0, 3.0, 4.0])\n",
    ));
    assert_ok(&prog(
        "    auto b = statistics.quantiles([1.0, 2.0, 3.0], n=3)\n",
    ));
    assert_ok(&prog(
        "    auto c = statistics.quantiles([1.0, 2.0, 3.0], method=\"inclusive\")\n",
    ));
    assert_ok(&prog(
        "    auto d = statistics.quantiles(xs=[1.0, 2.0, 3.0], n=4, method=\"exclusive\")\n",
    ));
    // Reordered keywords still land in the right slots.
    assert_ok(&prog(
        "    auto e = statistics.quantiles(method=\"inclusive\", xs=[1.0, 2.0, 3.0], n=5)\n",
    ));
}

#[test]
fn statistics_quantiles_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto a = statistics.quantiles([1.0, 2.0], buckets=4)\n"),
        "ZAP4023",
    );
}

#[test]
fn statistics_relationship_keywords() {
    // Two-list relationships name their lists `xs`/`ys`; `linear_regression`
    // carries the optional `proportional` flag.
    assert_ok(&prog(
        "    auto a = statistics.covariance(xs=[1.0, 2.0, 3.0], ys=[1.0, 2.0, 3.0])\n",
    ));
    assert_ok(&prog(
        "    auto b = statistics.correlation([1.0, 2.0, 3.0], [1.0, 2.0, 3.0])\n",
    ));
    assert_ok(&prog(
        "    auto c = statistics.linear_regression([1.0, 2.0, 3.0], [2.0, 4.0, 6.0])\n",
    ));
    assert_ok(&prog(
        "    auto d = statistics.linear_regression([1.0, 2.0], [2.0, 4.0], proportional=True)\n",
    ));
}

#[test]
fn statistics_linear_regression_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto a = statistics.linear_regression([1.0], [2.0], prop=True)\n"),
        "ZAP4023",
    );
}

#[test]
fn statistics_too_many_args_rejected() {
    assert_code(
        &prog("    auto a = statistics.mean([1.0, 2.0], [3.0, 4.0])\n"),
        "ZAP4025",
    );
}

// ---------------------------------------------------------------------------
// decimal (#981, #981) — the two seam constructors bind their single value by
// keyword. (The Decimal arithmetic / quantize / div surface is prelude-struct
// methods, so their keyword arguments ride method_param_info, not the seam.)
// ---------------------------------------------------------------------------

#[test]
fn decimal_constructor_keyword_binds() {
    assert_ok("import decimal\ndef f():\n    auto a = decimal.Decimal(value=\"1.50\")\n");
    assert_ok("import decimal\ndef f():\n    auto a = decimal.Decimal(value=42)\n");
    assert_ok("import decimal\ndef f():\n    auto a = decimal.from_float(f=1.25)\n");
}

#[test]
fn decimal_constructor_unknown_keyword_rejected() {
    assert_code(
        "import decimal\ndef f():\n    auto a = decimal.Decimal(val=\"1.50\")\n",
        "ZAP4023",
    );
}

#[test]
fn decimal_too_many_args_rejected() {
    assert_code(
        "import decimal\ndef f():\n    auto a = decimal.Decimal(\"1\", \"2\")\n",
        "ZAP4025",
    );
}

// ---------------------------------------------------------------------------
// fractions (#982) — the `Fraction` constructor binds `numerator` /
// `denominator` by keyword; `denominator` is the trailing optional dropped by
// the single-argument int / str forms. (The Fraction arithmetic /
// limit_denominator / to_float surface is prelude-struct methods, so their
// keyword arguments ride method_param_info, not the seam.)
// ---------------------------------------------------------------------------

#[test]
fn fractions_constructor_keyword_binds() {
    assert_ok(
        "import fractions\ndef f():\n    auto a = fractions.Fraction(numerator=3, denominator=4)\n",
    );
    assert_ok("import fractions\ndef f():\n    auto a = fractions.Fraction(numerator=5)\n");
    assert_ok("import fractions\ndef f():\n    auto a = fractions.Fraction(numerator=\"3/4\")\n");
    // reordered keywords bind by name, not position
    assert_ok(
        "import fractions\ndef f():\n    auto a = fractions.Fraction(denominator=4, numerator=3)\n",
    );
}

#[test]
fn fractions_constructor_unknown_keyword_rejected() {
    assert_code(
        "import fractions\ndef f():\n    auto a = fractions.Fraction(num=3, den=4)\n",
        "ZAP4023",
    );
}

#[test]
fn fractions_too_many_args_rejected() {
    assert_code(
        "import fractions\ndef f():\n    auto a = fractions.Fraction(1, 2, 3)\n",
        "ZAP4025",
    );
}

// ---------------------------------------------------------------------------
// collections (#972, epic #966) — Counter / defaultdict / OrderedDict.
//
// Unlike the `fractions`/`decimal` constructors (seam MODULE-facet functions
// bound by `bind_seam_args` + a `seam_sigs.rs` row), every collections
// convenience is a GENERIC prelude struct, so its `collections.Type[K](...)`
// spelling is seam-GENERIC sugar (D-0202) rewritten to a bare prelude free
// constructor (`new_counter` / `new_defaultdict` / `new_ordered_dict`) BEFORE
// module dispatch — exactly like `concurrent.Queue`. Keyword binding therefore
// comes from the ordinary prelude function/method machinery (I#298/I#481), NOT
// the seam registry, so these take NO `seam_sigs.rs` rows. These cases pin that
// the keyword surface (`items=` on the constructor, `n=` on `most_common`,
// `key=`/`n=` on `add`/`subtract`, `factory=` on `defaultdict`) binds by name.
// ---------------------------------------------------------------------------

#[test]
fn collections_counter_constructor_keyword_binds() {
    assert_ok(
        "import collections\ndef f():\n    auto c = collections.Counter[str](items=[\"a\", \"a\"])\n    print(c.total())\n",
    );
}

#[test]
fn collections_counter_method_keywords_bind() {
    assert_ok(
        "import collections\ndef f():\n    collections.Counter[str] c = collections.Counter()\n    c.add(key=\"a\", n=2)\n    c.subtract(key=\"a\", n=1)\n    auto top = c.most_common(n=1)\n    print(len(top))\n",
    );
}

#[test]
fn collections_defaultdict_constructor_keyword_binds() {
    assert_ok(
        "import collections\ndef zero() -> int64:\n    return 0\ndef f():\n    auto d = collections.defaultdict[str, int64](factory=zero)\n    print(len(d))\n",
    );
}

// ---------------------------------------------------------------------------
// dict/set initial-capacity hint (#1083) — `capacity=` constructor keyword +
// `reserve(n)` method.
//
// These are BUILT-IN collection constructors/methods, handled directly in the
// sema/lowering dispatch (like every `dict`/`set` form), NOT seam-module
// functions — there is no `dict`/`set` seam module, so they carry NO
// `seam_sigs.rs` row and do not flow through `bind_seam_args`. `capacity` is
// keyword-only by construction; `reserve`'s single arg stays POSITIONAL
// (Rust-style, one obvious arg). The acceptance/rejection behaviour is still
// pinned here alongside the other keyword-surface tests.
// ---------------------------------------------------------------------------

#[test]
fn dict_capacity_keyword_is_accepted() {
    assert_ok(&prog(
        "    dict[str, int64] d = dict[str, int64](capacity=200000)\n    print(len(d))\n",
    ));
}

#[test]
fn set_capacity_keyword_is_accepted() {
    assert_ok(&prog(
        "    set[str] s = set[str](capacity=1000)\n    print(len(s))\n",
    ));
}

#[test]
fn dict_capacity_zero_is_accepted() {
    // Zero is a valid hint (behaves as the default / no pre-size).
    assert_ok(&prog(
        "    dict[int64, int64] d = dict[int64, int64](capacity=0)\n    print(len(d))\n",
    ));
}

#[test]
fn dict_capacity_unknown_keyword_is_rejected() {
    // Only `capacity` is accepted; a misspelled keyword is the #998 footgun.
    assert_code(
        &prog("    auto d = dict[int64, int64](capcity=8)\n"),
        "ZAP4023",
    );
}

#[test]
fn dict_positional_capacity_is_rejected() {
    // The hint is keyword-only; a bare positional argument is rejected (a
    // populated dict uses a `{...}` literal).
    assert_code(&prog("    auto d = dict[int64, int64](8)\n"), "ZAP4025");
}

#[test]
fn dict_capacity_negative_literal_is_rejected() {
    assert_code(
        &prog("    auto d = dict[int64, int64](capacity=-1)\n"),
        "ZAP4024",
    );
}

#[test]
fn set_capacity_negative_literal_is_rejected() {
    assert_code(&prog("    auto s = set[int64](capacity=-5)\n"), "ZAP4024");
}

#[test]
fn dict_capacity_non_integer_is_rejected() {
    assert_code(
        &prog("    auto d = dict[int64, int64](capacity=\"big\")\n"),
        "ZAP4024",
    );
}

#[test]
fn set_still_accepts_positional_iterable() {
    // The capacity keyword must not break the existing `set[T](iterable)` form.
    assert_ok(&prog(
        "    set[int64] s = set[int64]([1, 2, 3])\n    print(len(s))\n",
    ));
}

#[test]
fn dict_reserve_positional_is_accepted() {
    assert_ok(&prog(
        "    dict[int64, int64] d = {}\n    d.reserve(1000)\n    print(len(d))\n",
    ));
}

#[test]
fn dict_reserve_negative_literal_is_rejected() {
    assert_code(
        &prog("    dict[int64, int64] d = {}\n    d.reserve(-1)\n"),
        "ZAP4024",
    );
}

#[test]
fn dict_reserve_wrong_arity_is_rejected() {
    assert_code(
        &prog("    dict[int64, int64] d = {}\n    d.reserve(1, 2)\n"),
        "ZAP4025",
    );
}

#[test]
fn dict_reserve_non_integer_is_rejected() {
    assert_code(
        &prog("    dict[int64, int64] d = {}\n    d.reserve(\"big\")\n"),
        "ZAP4024",
    );
}
