//! Seam calling-convention signature registry (#998, D-0204).
//!
//! Built-in (seam) module functions are hand-written check/lower pairs in
//! `zapc_sema::modules` and `zapc_ir::lower::modules` that read their arguments
//! **positionally**. Without a declared parameter list there is nowhere to bind
//! a keyword, no default to fill by name, and no name to validate — so a
//! misspelled keyword on a stdlib call was silently accepted and keyword
//! reordering bound by position (the #998 footgun).
//!
//! This module is the **single source of truth** for the declared parameter
//! names of seam methods, consumed by both facets:
//!
//!   * `zapc_sema::modules::bind_seam_args` — binds keywords by name, fills
//!     omitted trailing optionals, rejects an unknown keyword (`ZAP4023`, the
//!     same a user function emits), and hands the per-method checker the
//!     arguments normalized to declared positional order; and
//!   * `zapc_ir::lower::modules::reorder_seam_args` — the pure lowering twin
//!     that reorders the same call so the emitted runtime call gets the
//!     arguments in the right slots.
//!
//! It is kept in its **own file**, separate from the per-module check/lower
//! code and from [`crate::seam`] (which handles module-qualified generic
//! *construction*, D-0202): the per-module `.rs` files stay pure type/arity
//! logic, and this table is a flat, reviewable list of signatures. Adding
//! keyword/default support to a seam method is a single row here — the central
//! dispatch does the binding, so the per-method `check`/`lower` bodies are
//! untouched (they keep doing arity + type checks on the normalized args).
//!
//! **Convention for the rows.** Parameters are listed in declared positional
//! order; any optional parameters must come **last** (the binder rejects an
//! omitted parameter that is followed by a supplied one, so only trailing
//! parameters may be dropped — which matches every current seam method, whose
//! optionals are the trailing `level`/`headers`). A method with an *interior*
//! optional would need a richer per-parameter model; none exists today. A
//! method with **no** row keeps the legacy positional-only behavior (names
//! ignored), so migration is incremental and the eventual global default of
//! *"an un-listed method rejects keywords"* can be turned on once every
//! keyword-taking method has a row.
//!
//! **Authoring rule (D-0213): every NEW seam function with ≥1 argument gets a
//! row here — single-argument functions included.** Zap's call syntax is
//! Pythonic and some callers prefer named arguments (`fs.read_file(path=…)`), so
//! the whole stdlib surface is kept uniformly keyword-capable; a one-line row is
//! cheap. Only a **zero-argument** function has no row (nothing to name, e.g.
//! `fs.mkdtemp`, `time.monotonic`). Builtins (`print`, `input`, …) are not seam
//! functions and never appear here. This rule is forward-going; it does not force
//! an immediate back-fill of pre-D-0204 modules, but no new single-argument seam
//! function should ship rowless again.

/// `(module, method, declared positional parameter names)`. The module name is
/// the seam registry name as dispatched (`"zlib"`, `"gzip"`, `"http"`, …), i.e.
/// the same `module` string the sema/lowering dispatch receives.
pub const SEAM_METHOD_SIGS: &[(&str, &str, &[&str])] = &[
    // ---- zlib / gzip (ticket #106): compression with an optional level ----
    // Shared check/lower path, so both modules carry the same signatures.
    ("zlib", "compress", &["data", "level"]),
    ("zlib", "decompress", &["data"]),
    ("zlib", "crc32", &["data"]),
    ("zlib", "adler32", &["data"]),
    ("gzip", "compress", &["data", "level"]),
    ("gzip", "decompress", &["data"]),
    // ---- http (ticket #104): client verbs take an optional trailing headers ----
    // dict[str,str]; accessors are fixed-arity. The optional `headers` is the
    // last parameter (the former "n or n+1 args" overload).
    ("http", "get", &["url", "headers"]),
    ("http", "post", &["url", "body", "headers"]),
    ("http", "request", &["method", "url", "body", "headers"]),
    ("http", "status", &["resp"]),
    ("http", "body", &["resp"]),
    ("http", "header", &["resp", "name"]),
    ("http", "url", &["host", "port", "path"]),
    ("http", "read_request", &["conn"]),
    ("http", "method", &["req"]),
    ("http", "path", &["req"]),
    ("http", "req_header", &["req", "name"]),
    ("http", "req_headers", &["req"]),
    ("http", "req_body", &["req"]),
    ("http", "req_version", &["req"]),
    ("http", "respond", &["conn", "status", "body", "headers"]),
    // ---- datetime (ticket #99/#338): timestamp conversions + format/parse ----
    ("datetime", "from_timestamp", &["timestamp"]),
    ("datetime", "from_timestamp_local", &["timestamp"]),
    ("datetime", "to_timestamp", &["dt"]),
    // `format(dt)` is ISO-8601; `format(dt, fmt)` is the strftime overload — so
    // `fmt` is the trailing optional.
    ("datetime", "format", &["dt", "fmt"]),
    ("datetime", "strftime", &["dt", "fmt"]),
    ("datetime", "parse", &["s"]),
    ("datetime", "strptime", &["s", "fmt"]),
    // ---- random (ticket #98): only the non-numpy-overloaded verbs ----
    // `random`/`uniform`/`choice`/`shuffle` are overloaded with the NumPy
    // `Generator` surface (a distinct second signature keyed on an arg-0
    // `Generator` handle); a single module-form row cannot represent both and
    // would wrongly reject a NumPy keyword call, so they are deferred until the
    // registry grows a per-overload model. `seed`/`randint` are unambiguous.
    ("random", "seed", &["n"]),
    ("random", "randint", &["a", "b"]),
    // ---- base64 / hex / hash (tickets #337/#106): single data argument ----
    ("base64", "encode", &["data"]),
    ("base64", "encode_nopad", &["data"]),
    ("base64", "urlsafe_encode", &["data"]),
    ("base64", "urlsafe_encode_nopad", &["data"]),
    ("base64", "decode", &["data"]),
    ("base64", "urlsafe_decode", &["data"]),
    ("hex", "encode", &["data"]),
    ("hex", "decode", &["data"]),
    ("hash", "crc32c", &["data"]),
    // ---- json / yaml (tickets #100/#149): serialize / deserialize ----
    // Only the public surface; the `_`-prefixed lexer/encoder helpers are
    // internal and keep the legacy positional path.
    ("json", "loads", &["s"]),
    ("json", "scan", &["s"]),
    ("json", "dumps", &["v"]),
    ("yaml", "load", &["s"]),
    ("yaml", "dump", &["v"]),
    // ---- logging (ticket #101): level-tagged record emitters ----
    ("logging", "debug", &["msg"]),
    ("logging", "info", &["msg"]),
    ("logging", "warning", &["msg"]),
    ("logging", "error", &["msg"]),
    ("logging", "critical", &["msg"]),
    ("logging", "log", &["level", "msg"]),
    ("logging", "set_level", &["level"]),
    ("logging", "set_stream", &["stream"]),
    ("logging", "set_format", &["fmt"]),
    // ---- crypto (tickets #105/#762): KDFs + AEAD + constant-time compare ----
    // Parameter names and the trailing optionals are taken verbatim from each
    // function's own signature (its arity-error message), so they match the
    // documented surface; `dklen`/`salt`/`info`/`aad` are the trailing optionals.
    (
        "crypto",
        "pbkdf2_hmac",
        &["hash_name", "password", "salt", "iterations", "dklen"],
    ),
    (
        "crypto",
        "scrypt",
        &["password", "salt", "n", "r", "p", "dklen"],
    ),
    (
        "crypto",
        "hkdf",
        &["hash_name", "ikm", "length", "salt", "info"],
    ),
    ("crypto", "constant_time_eq", &["a", "b"]),
    (
        "crypto",
        "aes256gcm_encrypt",
        &["key", "nonce", "data", "aad"],
    ),
    (
        "crypto",
        "aes256gcm_decrypt",
        &["key", "nonce", "data", "aad"],
    ),
    (
        "crypto",
        "chacha20poly1305_encrypt",
        &["key", "nonce", "data", "aad"],
    ),
    (
        "crypto",
        "chacha20poly1305_decrypt",
        &["key", "nonce", "data", "aad"],
    ),
    // ---- os (ticket #97): environment + filesystem path helpers ----
    // (getpid/getcwd take no args, so no row.) These keyword names are newly
    // introduced (the seam checker used positional arity only) — reviewable.
    ("os", "getenv", &["name"]),
    ("os", "hasenv", &["name"]),
    ("os", "setenv", &["name", "value"]),
    ("os", "chdir", &["path"]),
    ("os", "mkdir", &["path"]),
    ("os", "remove", &["path"]),
    ("os", "rmdir", &["path"]),
    ("os", "listdir", &["path"]),
    ("os", "path_join", &["a", "b"]),
    ("os", "path_exists", &["path"]),
    ("os", "path_basename", &["path"]),
    ("os", "path_dirname", &["path"]),
    // filesystem gaps (#968, D-0211); environ/sep take no args, so no row.
    ("os", "path_isabs", &["path"]),
    ("os", "path_expanduser", &["path"]),
    ("os", "path_normpath", &["path"]),
    ("os", "path_abspath", &["path"]),
    ("os", "path_realpath", &["path"]),
    ("os", "path_relpath", &["path", "start"]),
    // ---- fs (ticket #97): file read/write + stat ----
    // (mkdtemp takes no args, so no row.)
    ("fs", "read_file", &["path"]),
    ("fs", "write_file", &["path", "data"]),
    ("fs", "exists", &["path"]),
    ("fs", "is_dir", &["path"]),
    ("fs", "is_file", &["path"]),
    ("fs", "size", &["path"]),
    ("fs", "listdir", &["path"]),
    // filesystem gaps (#968, D-0211).
    ("fs", "stat", &["path"]),
    ("fs", "mtime", &["path"]),
    ("fs", "rename", &["src", "dst"]),
    ("fs", "move", &["src", "dst"]),
    ("fs", "copy", &["src", "dst"]),
    ("fs", "makedirs", &["path"]),
    ("fs", "rmtree", &["path"]),
    ("fs", "read_bytes", &["path"]),
    ("fs", "write_bytes", &["path", "data"]),
    ("fs", "append", &["path", "data"]),
    // Line-oriented input (#969, D-0212): whole-file read split into lines.
    ("fs", "lines", &["path"]),
    // ---- time (ticket #99): sleeps + Duration constructors/accessors ----
    // (monotonic/monotonic_ns/time/time_ns take no args, so no row.)
    ("time", "sleep", &["seconds"]),
    ("time", "sleep_ns", &["ns"]),
    ("time", "seconds", &["n"]),
    ("time", "millis", &["n"]),
    ("time", "micros", &["n"]),
    ("time", "nanos", &["n"]),
    ("time", "to_seconds", &["d"]),
    ("time", "to_millis", &["d"]),
    ("time", "to_micros", &["d"]),
    // ---- process (ticket #97) ----
    ("process", "run", &["cmd"]),
    // ---- sys (ticket #97): the single-argument verbs (argv/platform/*_fd/
    // cpu_source/cpu_sve_vl take no args, so no row) ----
    ("sys", "exit", &["code"]),
    ("sys", "cpu_has", &["name"]),
    // ---- url (ticket #335): percent-encoding + query builders ----
    ("url", "quote", &["s"]),
    ("url", "quote_path", &["s"]),
    ("url", "unquote", &["s"]),
    ("url", "unquote_plus", &["s"]),
    ("url", "encode_query", &["d"]),
    ("url", "canonical_query", &["d"]),
    // url parse / build (#970, D-0209). `join` is the one that materially
    // benefits from keywords — two same-typed `str` args, so `url.join(base=...,
    // ref=...)` reads clearly and guards against an accidental swap; the rest
    // are single-argument and rowed only for a uniform keyword-capable surface.
    ("url", "split", &["s"]),
    ("url", "build", &["parts"]),
    ("url", "parse_query", &["q"]),
    ("url", "parse_query_first", &["q"]),
    ("url", "encode_query_multi", &["d"]),
    ("url", "join", &["base", "ref"]),
    // ---- io (ticket #97): raw fd I/O ----
    ("io", "open", &["path", "mode"]),
    ("io", "read", &["fd", "n"]),
    ("io", "write", &["fd", "data"]),
    ("io", "close_fd", &["fd"]),
    // Buffered line-oriented input (#969, D-0212). `readn` materially benefits
    // from keywords (`io.readn(reader=r, n=8)`); the single-argument reader
    // verbs are rowed only for a uniform keyword-capable surface. The reader
    // handle parameter is named `reader`. (`input` is a global builtin, not a
    // seam function, so it has no row — like `print`.)
    ("io", "reader", &["fd"]),
    ("io", "readline", &["reader"]),
    ("io", "readall", &["reader"]),
    ("io", "readn", &["reader", "n"]),
    // ---- signal (ticket #77): subscribe to a Signal ----
    ("signal", "notify", &["sig"]),
    // ---- math (ticket #98): scalar transcendentals + 2/3-arg forms ----
    // The 1-arg funcs each take `x` (a scalar, or element-wise over an array —
    // same arity/param, so one row covers both). gc/runtime take no args.
    ("math", "sqrt", &["x"]),
    ("math", "exp", &["x"]),
    ("math", "log", &["x"]),
    ("math", "log2", &["x"]),
    ("math", "log10", &["x"]),
    ("math", "sin", &["x"]),
    ("math", "cos", &["x"]),
    ("math", "tan", &["x"]),
    ("math", "asin", &["x"]),
    ("math", "acos", &["x"]),
    ("math", "atan", &["x"]),
    ("math", "floor", &["x"]),
    ("math", "ceil", &["x"]),
    ("math", "round", &["x"]),
    ("math", "trunc", &["x"]),
    ("math", "fabs", &["x"]),
    ("math", "isnan", &["x"]),
    ("math", "isinf", &["x"]),
    ("math", "pow", &["x", "y"]),
    ("math", "atan2", &["y", "x"]), // Python order: atan2(y, x)
    ("math", "hypot", &["x", "y"]),
    ("math", "fmod", &["x", "y"]),
    ("math", "fma", &["x", "y", "z"]),
    // ---- net (ticket #104): plaintext TCP over opaque socket handles ----
    // (connect_ip_deadline is left on the legacy path pending a settled name
    // for its deadline parameter.)
    ("net", "listen", &["host", "port"]),
    ("net", "connect", &["host", "port"]),
    ("net", "accept", &["listener"]),
    ("net", "recv", &["conn", "n"]),
    ("net", "send", &["conn", "data"]),
    ("net", "port", &["listener"]),
    ("net", "set_deadline_ns", &["conn", "ns"]),
    ("net", "set_read_timeout", &["conn", "ms"]),
    ("net", "set_read_deadline", &["conn", "ms"]),
    ("net", "close_conn", &["conn"]),
    ("net", "close_listener", &["listener"]),
    // ---- linalg (tickets #926-#928): LAPACK-backed linear algebra (NumPy names) ----
    ("linalg", "solve", &["a", "b"]),
    ("linalg", "inv", &["a"]),
    ("linalg", "det", &["a"]),
    ("linalg", "cholesky", &["a"]),
    ("linalg", "qr", &["a"]),
    ("linalg", "svd", &["a"]),
    ("linalg", "eigvals", &["a"]),
    ("linalg", "eig", &["a"]),
    ("linalg", "eigh", &["a"]),
    ("linalg", "eigvalsh", &["a"]),
    ("linalg", "lstsq", &["a", "b"]),
    ("linalg", "pinv", &["a"]),
    ("linalg", "matrix_power", &["a", "n"]),
    ("linalg", "norm", &["x", "ord"]),
    // ---- tls (tickets #147/#488/#560): client/server TLS; ctx/capath/keyfile ----
    // optional trailing (Python `ssl` names; host/port/conn/n/data as in `net`).
    ("tls", "connect", &["host", "port", "ctx"]),
    ("tls", "connect_insecure", &["host", "port"]),
    ("tls", "load_verify_locations", &["ctx", "cafile", "capath"]),
    ("tls", "load_cert_chain", &["ctx", "certfile", "keyfile"]),
    ("tls", "listen", &["host", "port", "cert_path", "key_path"]),
    ("tls", "recv", &["conn", "n"]),
    ("tls", "send", &["conn", "data"]),
    // ---- argparse (ticket #101): global singleton parser; `default` trailing ----
    ("argparse", "set_prog", &["prog"]),
    ("argparse", "set_description", &["description"]),
    ("argparse", "add_flag", &["name"]),
    ("argparse", "add_positional", &["name"]),
    ("argparse", "add_option", &["name", "default"]),
    ("argparse", "add_int", &["name", "default"]),
    ("argparse", "parse", &["argv"]),
    ("argparse", "get", &["name"]),
    ("argparse", "get_int", &["name"]),
    ("argparse", "get_flag", &["name"]),
    // ---- xml (ticket #351): DOM parse / serialize / construct ----
    ("xml", "parse", &["s"]),
    ("xml", "dumps", &["e"]),
    ("xml", "element", &["name"]),
    // ---- concurrent (ticket #184): lock-free Counter; `start` optional ----
    ("concurrent", "Counter", &["start"]),
    // ---- bisect (ticket #102): binary search over a sorted list (Python names) ----
    ("bisect", "bisect_left", &["a", "x"]),
    ("bisect", "bisect_right", &["a", "x"]),
    ("bisect", "insort_left", &["a", "x"]),
    ("bisect", "insort_right", &["a", "x"]),
];

/// The declared positional parameter names of a seam method, or `None` when the
/// method has no registered signature (then the caller keeps the legacy
/// positional-only behavior). The `(module, method)` key is the seam registry
/// name the dispatch uses.
pub fn seam_method_params(module: &str, method: &str) -> Option<&'static [&'static str]> {
    SEAM_METHOD_SIGS
        .iter()
        .find(|(m, me, _)| *m == module && *me == method)
        .map(|(_, _, params)| *params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn lookup_hits_and_misses() {
        assert_eq!(
            seam_method_params("zlib", "compress"),
            Some(&["data", "level"][..])
        );
        assert_eq!(
            seam_method_params("http", "get"),
            Some(&["url", "headers"][..])
        );
        assert_eq!(seam_method_params("zlib", "no_such"), None);
        assert_eq!(seam_method_params("no_such", "compress"), None);
    }

    /// A `(module, method)` row is unique (a duplicate would make `find` ignore
    /// the later one silently).
    #[test]
    fn rows_are_unique() {
        let mut seen = BTreeSet::new();
        for (m, me, _) in SEAM_METHOD_SIGS {
            assert!(
                seen.insert((*m, *me)),
                "duplicate seam signature row: {m}.{me}"
            );
        }
    }

    /// Parameter names within one method are distinct (a repeat would make a
    /// keyword bind to the wrong slot / never match).
    #[test]
    fn params_distinct_within_a_method() {
        for (m, me, params) in SEAM_METHOD_SIGS {
            let mut seen = BTreeSet::new();
            for p in *params {
                assert!(seen.insert(*p), "duplicate parameter '{p}' in {m}.{me}");
            }
        }
    }
}
