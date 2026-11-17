/* zap runtime fragment: tls.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * tls (ticket #147, D-0062). A TLS *client* over the `net` TCP sockets, backed
 * by system OpenSSL (`-lssl -lcrypto`) and integrated with the netpoller (#80):
 * the fd is non-blocking, and SSL_connect / SSL_read / SSL_write returning
 * WANT_READ / WANT_WRITE PARK the coroutine via `zap_sock_park` (freeing its
 * worker M) instead of blocking an OS thread or busy-polling. `tls.connect`
 * verifies the certificate chain against the system trust store AND the hostname
 * by default (SSL_set1_host + SSL_VERIFY_PEER); `tls.connect_insecure` is the
 * loudly-named opt-out (never the default).
 *
 * Only the unified post-1.1.0 OpenSSL API is used (TLS_client_method, automatic
 * library init, SSL_CTX_set_min_proto_version, SSL_set1_host), identical across
 * OpenSSL 1.1.1 / 3.x and LibreSSL >= 2.7, so no version #ifdef is needed; the
 * CI image is OpenSSL 3.0.13 on Ubuntu 24.04. The public TLS **server**
 * (`tls.listen`/`accept`/`port`/`close_listener`, ticket #488, D-0093) lives
 * just below the client; it loads a cert+key from disk and runs the server-side
 * handshake over the same netpoller machinery. The self-signed loopback echo
 * server behind `tls._test_server()` remains an internal test scaffold (its cert
 * gen uses the OpenSSL 3.0 API, the CI target).
 *
 * GC discipline mirrors the net section: the `ZapTls` handle is ATOMIC (fd is a
 * scalar; pd / SSL* / SSL_CTX* are non-GC infra), fd/pd/SSL* are pulled into C
 * locals before any park, payloads are copied to malloc scratch, and GC results
 * are built only after the final park. The socket BIO is BIO_NOCLOSE, so
 * the runtime owns the fd and closes it exactly once via `zap_sock_shutdown`.
 * ========================================================================== */
/* OpenSSL backs `tls`, the OpenSSL-based `crypto` primitives, and the http
 * `https://` client. #853 links `-lssl -lcrypto` and compiles this whole region
 * — the headers + OpenSSL-backed crypto block + the TLS transport setup here,
 * and (in a second guard below, past the shared connection helpers) the TLS
 * client/server + the http client/server — ONLY when the program reaches a
 * TLS/crypto/http seam. A program that reaches none is built `-DZAP_NO_OPENSSL`
 * and this is compiled out, referencing no OpenSSL symbol, so no libssl/libcrypto
 * links. Unlike CPython there are NO stubs: the gated entry points are simply
 * absent, because such a program never calls them (the self-contained
 * SHA/MD5/HMAC/CSPRNG `zap_crypto_*` live above this region and stay compiled).
 * See `program_backend_use` in crates/codegen/src/lib.rs. */
