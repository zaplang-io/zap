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
    // dict[str,str] (#280) and an optional per-request total-deadline `timeout`
    // in milliseconds (#986, D-0229). `headers` is an *interior* optional (a
    // later `timeout` may be supplied without it, e.g. `http.get(url,
    // timeout=5000)`): it carries a `SeamDefault::EmptyDict` in
    // `SEAM_METHOD_DEFAULTS` so the binder fills the hole with `{}` instead of
    // rejecting it. `timeout` is the trailing optional (0 / omitted = unbounded).
    ("http", "get", &["url", "headers", "timeout"]),
    ("http", "post", &["url", "body", "headers", "timeout"]),
    (
        "http",
        "request",
        &["method", "url", "body", "headers", "timeout"],
    ),
    ("http", "status", &["resp"]),
    ("http", "body", &["resp"]),
    ("http", "header", &["resp", "name"]),
    // redirect-following result accessors (#987). Single-argument, rowed for a
    // uniform keyword-capable surface (D-0213).
    ("http", "final_url", &["resp"]),
    ("http", "redirects", &["resp"]),
    // client response header enumeration + Set-Cookie parsing (#989). Single-arg,
    // rowed for a uniform keyword-capable surface (D-0213).
    ("http", "headers", &["resp"]),
    ("http", "parse_set_cookie", &["value"]),
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
    // Arithmetic / comparisons / field helpers (#974, D-0221).
    ("datetime", "add", &["dt", "delta"]),
    ("datetime", "diff", &["a", "b"]),
    ("datetime", "before", &["a", "b"]),
    ("datetime", "after", &["a", "b"]),
    ("datetime", "equal", &["a", "b"]),
    ("datetime", "weekday", &["dt"]),
    ("datetime", "isoweekday", &["dt"]),
    ("datetime", "day_of_year", &["dt"]),
    // ---- datetime timezones (#1045, D-0225) ----
    ("datetime", "zone", &["name"]),
    ("datetime", "fixed_offset", &["minutes"]),
    ("datetime", "astimezone", &["dt", "tz"]),
    ("datetime", "isoformat", &["dt"]),
    ("datetime", "rfc3339", &["dt"]),
    ("datetime", "rfc3339_parse", &["s"]),
    // ---- datetime date/time types (#1046, D-0226) ----
    // NOTE: `replace` is intentionally NOT listed — its fields are all-optional,
    // any-subset keywords (interior holes), which the trailing-only seam binder
    // (D-0204) rejects, so `replace` binds its keywords itself.
    ("datetime", "date", &["year", "month", "day"]),
    ("datetime", "time", &["hour", "minute", "second"]),
    ("datetime", "to_date", &["dt"]),
    ("datetime", "to_time", &["dt"]),
    ("datetime", "combine", &["date", "time"]),
    // Date field helpers (#1055).
    ("datetime", "date_weekday", &["d"]),
    ("datetime", "date_isoweekday", &["d"]),
    ("datetime", "date_day_of_year", &["d"]),
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
    // Tuple-returning decomposition (#1030, D-0219).
    ("os", "path_split", &["path"]),
    ("os", "path_splitext", &["path"]),
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
    ("fs", "open_reader", &["path"]),
    ("fs", "scandir", &["path"]),
    ("fs", "walk", &["root"]),
    ("fs", "copytree", &["src", "dst"]),
    // Path matching (#1033, D-0216).
    ("fs", "fnmatch", &["name", "pattern"]),
    ("fs", "glob", &["root", "pattern"]),
    // Tempfile (#979, D-0239). `mkstemp`'s three args are all optional str
    // kwargs; `prefix`/`suffix` carry a `SeamDefault::Str` below so an interior
    // hole (e.g. `fs.mkstemp(dir=…)`) fills by name. `gettempdir` takes no args
    // (no row). `write_atomic`'s two args are required.
    ("fs", "mkstemp", &["prefix", "suffix", "dir"]),
    ("fs", "write_atomic", &["path", "data"]),
    // temp_file (#1077): four optional args (all recommended keyword). Each of
    // prefix/suffix/dir/delete carries a `SeamDefault` below so an interior hole
    // (e.g. `fs.temp_file(delete=False)`) fills by name. `_unlink_quiet` is the
    // internal 1-arg close helper — positional, so no row.
    ("fs", "temp_file", &["prefix", "suffix", "dir", "delete"]),
    // ---- time (ticket #99): sleeps + Duration constructors/accessors ----
    // (monotonic/monotonic_ns/time/time_ns take no args, so no row.)
    ("time", "sleep", &["seconds"]),
    ("time", "sleep_ns", &["ns"]),
    ("time", "sleep_for", &["d"]),
    ("time", "format", &["d"]),
    ("time", "since", &["start"]),
    ("time", "seconds", &["n"]),
    ("time", "millis", &["n"]),
    ("time", "micros", &["n"]),
    ("time", "nanos", &["n"]),
    ("time", "to_seconds", &["d"]),
    ("time", "to_millis", &["d"]),
    ("time", "to_micros", &["d"]),
    // ---- process (ticket #97) ----
    // `cmd` is the positional argv (list[str]); every other option is a keyword
    // mirroring Python `subprocess.run(..., env=, cwd=, timeout=, input=,
    // capture_output=)` (#976, D-0237). `check_run` (Python `check=True`) and the
    // explicit shell form `run_shell` (whose `cmd` is a `str`) share the option
    // set. Interior holes fill from SEAM_METHOD_DEFAULTS; `merge_stderr` is the
    // only trailing-optional with no default row (left absent when omitted).
    (
        "process",
        "run",
        &[
            "cmd",
            "env",
            "cwd",
            "timeout",
            "input",
            "capture_output",
            "merge_stderr",
        ],
    ),
    (
        "process",
        "check_run",
        &[
            "cmd",
            "env",
            "cwd",
            "timeout",
            "input",
            "capture_output",
            "merge_stderr",
        ],
    ),
    (
        "process",
        "run_shell",
        &[
            "cmd",
            "env",
            "cwd",
            "timeout",
            "input",
            "capture_output",
            "merge_stderr",
        ],
    ),
    // ---- sys (ticket #97): the single-argument verbs (argv/platform/*_fd/
    // cpu_source/cpu_sve_vl take no args, so no row) ----
    ("sys", "exit", &["code"]),
    ("sys", "cpu_has", &["name"]),
    // Standard-stream write (#1035, D-0217); stdin/stdout/stderr take no args.
    ("sys", "write", &["stream", "data"]),
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
    // ---- uuid (ticket #975, D-0232/D-0233): namespaced UUIDs + parse ----
    // The generators (uuid4/uuid7/uuid1/nil/max/namespace_*) take no args, so no
    // row. `nanoid` stays positional (its 0/1/2-arg forms bind different
    // parameters, which the positional binder cannot express). `uuid3`/`uuid5`
    // and `parse` are keyword-capable.
    ("uuid", "uuid3", &["ns", "name"]),
    ("uuid", "uuid5", &["ns", "name"]),
    ("uuid", "parse", &["s"]),
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
    ("io", "lines", &["reader"]),
    // #1071, D-0240: io.writer(fd) -> Writer. The flat `_writer_*` delegates are
    // internal (called only by the prelude `Writer`), so — like the threading
    // `_*` delegates — they get no row and stay positional.
    ("io", "writer", &["fd"]),
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
    // ---- math (#973, D-0234): everyday scalar additions ----
    // Unary f64 -> f64 (angles, hyperbolics, accurate variants, special funcs).
    ("math", "degrees", &["x"]),
    ("math", "radians", &["x"]),
    ("math", "sinh", &["x"]),
    ("math", "cosh", &["x"]),
    ("math", "tanh", &["x"]),
    ("math", "asinh", &["x"]),
    ("math", "acosh", &["x"]),
    ("math", "atanh", &["x"]),
    ("math", "expm1", &["x"]),
    ("math", "log1p", &["x"]),
    ("math", "exp2", &["x"]),
    ("math", "cbrt", &["x"]),
    ("math", "gamma", &["x"]),
    ("math", "lgamma", &["x"]),
    ("math", "erf", &["x"]),
    ("math", "erfc", &["x"]),
    ("math", "isfinite", &["x"]),
    ("math", "modf", &["x"]),
    ("math", "frexp", &["x"]),
    // Binary / compose forms.
    ("math", "copysign", &["x", "y"]),
    ("math", "nextafter", &["x", "y"]),
    ("math", "remainder", &["x", "y"]),
    ("math", "ldexp", &["x", "i"]),
    // isclose(a, b, rel_tol=1e-9, abs_tol=0.0): the tolerances are optional
    // keyword args with defaults (see `SEAM_METHOD_DEFAULTS`).
    ("math", "isclose", &["a", "b", "rel_tol", "abs_tol"]),
    // Integer math (int -> int).
    ("math", "gcd", &["a", "b"]),
    ("math", "lcm", &["a", "b"]),
    ("math", "factorial", &["n"]),
    ("math", "comb", &["n", "k"]),
    ("math", "perm", &["n", "k"]),
    ("math", "isqrt", &["n"]),
    // List reductions.
    ("math", "prod", &["xs"]),
    ("math", "fsum", &["xs"]),
    ("math", "dist", &["p", "q"]),
    ("math", "sumprod", &["p", "q"]),
    // ---- statistics (#980, D-0243): descriptive stats over a scalar list ----
    // Means + order statistics take one list; the list arg is named `xs`.
    ("statistics", "mean", &["xs"]),
    ("statistics", "fmean", &["xs"]),
    ("statistics", "geometric_mean", &["xs"]),
    ("statistics", "harmonic_mean", &["xs"]),
    ("statistics", "median", &["xs"]),
    ("statistics", "median_low", &["xs"]),
    ("statistics", "median_high", &["xs"]),
    ("statistics", "mode", &["xs"]),
    ("statistics", "multimode", &["xs"]),
    // Spread with an optional trailing centre (`xbar`/`mu`), left absent when
    // omitted (filled in lowering) — no interior hole, so no default row.
    ("statistics", "variance", &["xs", "xbar"]),
    ("statistics", "pvariance", &["xs", "mu"]),
    ("statistics", "stdev", &["xs", "xbar"]),
    ("statistics", "pstdev", &["xs", "mu"]),
    // quantiles(xs, n=4, method='exclusive'): `n` sits before the trailing
    // `method`, so it carries a SeamDefault::Int(4) below for `quantiles(xs,
    // method='inclusive')`. `method` is trailing (defaulted in lowering).
    ("statistics", "quantiles", &["xs", "n", "method"]),
    // Two-list relationships; `proportional` is a trailing optional.
    ("statistics", "covariance", &["xs", "ys"]),
    ("statistics", "correlation", &["xs", "ys"]),
    (
        "statistics",
        "linear_regression",
        &["xs", "ys", "proportional"],
    ),
    // ---- decimal (#981, #981): exact base-10 Decimal constructors ----
    // Both take a single positional value, named for the keyword form
    // (`decimal.Decimal(value="1.50")`). The `Decimal` arithmetic/`quantize`/
    // `div` surface is prelude-struct methods (kwargs via method_param_info,
    // I#481), not seam functions, so they carry no rows here.
    ("decimal", "Decimal", &["value"]),
    ("decimal", "from_float", &["f"]),
    // ---- fractions (#982): exact rational Fraction constructor ----
    // `Fraction(numerator, denominator)` — `denominator` is the trailing
    // optional (omitted for the single-argument `Fraction(int)` / `Fraction(str)`
    // forms), so `fractions.Fraction(numerator=3, denominator=4)` and
    // `fractions.Fraction(numerator="3/4")` both bind by name. All `Fraction`
    // arithmetic / `limit_denominator` / `to_float` / comparison surface is
    // prelude-struct methods (kwargs via method_param_info, I#481), not seam
    // functions, so they carry no rows here.
    ("fractions", "Fraction", &["numerator", "denominator"]),
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
    // UDP datagram sockets (#991, epic #985). The 0-arg constructors
    // (udp_socket / udp_socket6) carry no params, so they need no kwargs row.
    ("net", "udp_bind", &["sock", "host", "port"]),
    ("net", "udp_connect", &["sock", "host", "port"]),
    ("net", "sendto", &["sock", "data", "host", "port"]),
    ("net", "recvfrom", &["sock", "n"]),
    // DNS resolution surface (#992, epic #985).
    ("net", "lookup_host", &["name"]),
    ("net", "lookup_ipv4", &["name"]),
    ("net", "lookup_ipv6", &["name"]),
    ("net", "lookup_addr", &["ip"]),
    // Unix domain sockets (#993, epic #985). The 0-arg unixgram_socket carries
    // no params, so it needs no kwargs row.
    ("net", "listen_unix", &["path"]),
    ("net", "connect_unix", &["path"]),
    ("net", "unixgram_bind", &["sock", "path"]),
    ("net", "sendto_unix", &["sock", "data", "path"]),
    ("net", "recvfrom_unix", &["sock", "n"]),
    // Socket options (#994, epic #985).
    ("net", "set_reuseaddr", &["sock", "on"]),
    ("net", "set_reuseport", &["sock", "on"]),
    ("net", "set_nodelay", &["sock", "on"]),
    ("net", "set_keepalive", &["sock", "on"]),
    ("net", "set_recv_buffer", &["sock", "nbytes"]),
    ("net", "set_send_buffer", &["sock", "nbytes"]),
    ("net", "get_nodelay", &["sock"]),
    ("net", "get_keepalive", &["sock"]),
    ("net", "get_recv_buffer", &["sock"]),
    ("net", "get_send_buffer", &["sock"]),
    // TCP keepalive interval tuning (#1141, follow-up to #994).
    ("net", "set_keepalive_idle", &["sock", "seconds"]),
    ("net", "set_keepalive_interval", &["sock", "seconds"]),
    ("net", "set_keepalive_count", &["sock", "count"]),
    ("net", "get_keepalive_idle", &["sock"]),
    ("net", "get_keepalive_interval", &["sock"]),
    ("net", "get_keepalive_count", &["sock"]),
    // UDP multicast (#1142, follow-up to #991/#994).
    ("net", "join_multicast", &["sock", "group"]),
    ("net", "leave_multicast", &["sock", "group"]),
    ("net", "set_multicast_ttl", &["sock", "hops"]),
    ("net", "set_multicast_loopback", &["sock", "on"]),
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
    // The object-model constructor (#997, D-0238): both arguments optional, so
    // each carries a SeamDefault::Str("") below for keyword-with-a-hole calls
    // (e.g. `ArgumentParser(description="x")` fills `prog`).
    ("argparse", "ArgumentParser", &["prog", "description"]),
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
    // ---- heap (ticket #978): top-K helpers (Python heapq.nlargest/nsmallest) ----
    // `n` and `iterable` are naturally positional (count then sequence, the
    // Python order); `key` is the trailing optional selector, usually passed as
    // `key=`. All three are keyword-capable (D-0213).
    ("heap", "nlargest", &["n", "iterable", "key"]),
    ("heap", "nsmallest", &["n", "iterable", "key"]),
];

/// A default value a seam parameter takes when it is an *interior* optional —
/// omitted while a later parameter is supplied (#986). Without a default, such a
/// hole is rejected (the #998 footgun guard); with one, the binder and its
/// lowering twin synthesize the value so the per-method facet, which reads
/// positionally, still finds every slot filled. Kept tiny and explicit (one
/// variant per shape actually needed) rather than a general expression default:
/// the only consumer today is `http`'s interior `headers` dict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeamDefault {
    /// An empty `dict` display (`{}`): the default for an omitted `headers` map,
    /// which the runtime treats as "no caller headers".
    EmptyDict,
    /// A `float` literal, carried as its raw source lexeme (e.g. `"1e-9"`) so it
    /// types + lowers exactly like the literal a caller would have written. The
    /// default for `math.isclose`'s interior `rel_tol` (#973, D-0234).
    Float(&'static str),
    /// An `int` literal, carried as its raw source lexeme (e.g. `"4"`) so it
    /// types + lowers exactly like the literal a caller would have written. The
    /// default for `statistics.quantiles`'s interior `n` (#980, D-0243).
    Int(&'static str),
    /// A `str` literal, carried as its raw (un-escaped) UTF-8 contents. Lowers
    /// exactly like a plain string literal a caller would have written. The
    /// default for `process.run`'s interior `cwd` / `input` (#976, D-0237).
    Str(&'static str),
    /// A `bool` literal (`True` / `False`). The default for `process.run`'s
    /// interior `capture_output` / `merge_stderr` (#976, D-0237).
    Bool(bool),
}

/// Parameters that carry a [`SeamDefault`], keyed by `(module, method,
/// parameter)`. A parameter absent from this table has no default: if it is an
/// interior hole the binder rejects the call as before. Only interior optionals
/// need an entry — a trailing optional is simply left absent when omitted.
pub const SEAM_METHOD_DEFAULTS: &[(&str, &str, &str, SeamDefault)] = &[
    // `argparse.ArgumentParser(prog="", description="")` (#997, D-0238): both
    // optional, so `prog` must default when a caller supplies only
    // `description=` by keyword (`description` is trailing, filled in lowering).
    ("argparse", "ArgumentParser", "prog", SeamDefault::Str("")),
    (
        "argparse",
        "ArgumentParser",
        "description",
        SeamDefault::Str(""),
    ),
    // `fs.mkstemp(prefix="tmp", suffix="", dir="")` (#979, D-0239): `prefix` and
    // `suffix` precede the trailing `dir`, so each must default when a caller
    // supplies only a later one by keyword (`fs.mkstemp(dir="/var/tmp")`). `dir`
    // is trailing — left absent when omitted, resolved to the temp dir at runtime.
    ("fs", "mkstemp", "prefix", SeamDefault::Str("tmp")),
    ("fs", "mkstemp", "suffix", SeamDefault::Str("")),
    // temp_file (#1077): same prefix/suffix/dir defaults as mkstemp, plus
    // `delete` defaulting to True (Python's `NamedTemporaryFile` default). All
    // four carry a default so any later keyword (e.g. `delete=False`) fills the
    // interior holes before it.
    ("fs", "temp_file", "prefix", SeamDefault::Str("tmp")),
    ("fs", "temp_file", "suffix", SeamDefault::Str("")),
    ("fs", "temp_file", "dir", SeamDefault::Str("")),
    ("fs", "temp_file", "delete", SeamDefault::Bool(true)),
    // `http` client verbs: `headers` sits before the trailing `timeout` (#986),
    // so it must default when a caller supplies only `timeout`.
    ("http", "get", "headers", SeamDefault::EmptyDict),
    ("http", "post", "headers", SeamDefault::EmptyDict),
    ("http", "request", "headers", SeamDefault::EmptyDict),
    // `math.isclose(a, b, rel_tol=1e-9, abs_tol=0.0)`: `rel_tol` sits before the
    // trailing `abs_tol`, so it must default when a caller supplies only
    // `abs_tol` (`isclose(a, b, abs_tol=0.1)`). `abs_tol` is trailing and is
    // filled in lowering, but is listed for symmetry (#973, D-0234).
    ("math", "isclose", "rel_tol", SeamDefault::Float("1e-9")),
    ("math", "isclose", "abs_tol", SeamDefault::Float("0.0")),
    // `statistics.quantiles(xs, n=4, method='exclusive')`: `n` precedes the
    // trailing `method`, so it must default when a caller supplies only
    // `method=` by keyword (#980, D-0243). `method` is trailing (filled in
    // lowering), listed for symmetry.
    ("statistics", "quantiles", "n", SeamDefault::Int("4")),
    (
        "statistics",
        "quantiles",
        "method",
        SeamDefault::Str("exclusive"),
    ),
    // `process.run(cmd, env={}, cwd="", timeout=0.0, input="",
    // capture_output=True, merge_stderr=False)` and its `check_run` / `run_shell`
    // siblings (#976, D-0237): each option before the last one supplied must
    // default. `merge_stderr` is trailing (no row — left absent when omitted).
    ("process", "run", "env", SeamDefault::EmptyDict),
    ("process", "run", "cwd", SeamDefault::Str("")),
    ("process", "run", "timeout", SeamDefault::Float("0.0")),
    ("process", "run", "input", SeamDefault::Str("")),
    ("process", "run", "capture_output", SeamDefault::Bool(true)),
    ("process", "check_run", "env", SeamDefault::EmptyDict),
    ("process", "check_run", "cwd", SeamDefault::Str("")),
    ("process", "check_run", "timeout", SeamDefault::Float("0.0")),
    ("process", "check_run", "input", SeamDefault::Str("")),
    (
        "process",
        "check_run",
        "capture_output",
        SeamDefault::Bool(true),
    ),
    ("process", "run_shell", "env", SeamDefault::EmptyDict),
    ("process", "run_shell", "cwd", SeamDefault::Str("")),
    ("process", "run_shell", "timeout", SeamDefault::Float("0.0")),
    ("process", "run_shell", "input", SeamDefault::Str("")),
    (
        "process",
        "run_shell",
        "capture_output",
        SeamDefault::Bool(true),
    ),
];

/// The [`SeamDefault`] for `(module, method, param)`, or `None` when the
/// parameter has no default (an interior hole there is an error, as before).
pub fn seam_param_default(module: &str, method: &str, param: &str) -> Option<SeamDefault> {
    SEAM_METHOD_DEFAULTS
        .iter()
        .find(|(m, me, p, _)| *m == module && *me == method && *p == param)
        .map(|(_, _, _, d)| *d)
}

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
            Some(&["url", "headers", "timeout"][..])
        );
        assert_eq!(seam_method_params("zlib", "no_such"), None);
        assert_eq!(seam_method_params("no_such", "compress"), None);
    }

    /// An interior-optional parameter resolves to its default; unmarked
    /// parameters (and methods) have none (#986).
    #[test]
    fn interior_optional_defaults() {
        assert_eq!(
            seam_param_default("http", "get", "headers"),
            Some(SeamDefault::EmptyDict)
        );
        assert_eq!(
            seam_param_default("http", "request", "headers"),
            Some(SeamDefault::EmptyDict)
        );
        assert_eq!(seam_param_default("http", "get", "timeout"), None);
        assert_eq!(seam_param_default("http", "get", "url"), None);
        assert_eq!(seam_param_default("zlib", "compress", "level"), None);
    }

    /// Every default row names a real parameter of its method (a typo would make
    /// the binder never find the default and silently reject the hole).
    #[test]
    fn default_rows_name_real_params() {
        for (m, me, p, _) in SEAM_METHOD_DEFAULTS {
            let params = seam_method_params(m, me)
                .unwrap_or_else(|| panic!("default row for unknown seam method {m}.{me}"));
            assert!(
                params.contains(p),
                "default row {m}.{me} names parameter '{p}' not in its signature"
            );
        }
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
