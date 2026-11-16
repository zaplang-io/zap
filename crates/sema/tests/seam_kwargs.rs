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
