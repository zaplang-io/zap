/* Numeric acceleration seam (#912, D-0179) — part of the array performance &
 * acceleration epic (#914). Provider-neutral hooks the array dispatcher (#904)
 * consults to choose between the own-kernel FLOOR (simple, exact, always linked)
 * and a BLAS/LAPACK accelerator CEILING (auto-used when present).
 *
 * The provider is chosen at BUILD time — OpenBLAS on Linux, Apple Accelerate on
 * macOS (one cblas/lapacke ABI) — and a program links it only when it reaches an
 * accel-eligible op (#904), exactly like the pcre2/zlib seams: the codegen
 * defines `ZAP_HAVE_BLAS` (plus the provider macro) on the runtime compile and
 * adds the provider's link flags only for such a program. There is no opt-out
 * knob (#925 dropped `ZAP_NO_BLAS`): a program that uses no accel-eligible op
 * links no provider anyway, and `ZAP_ACCEL` is the runtime override. When no
 * provider is compiled in (an exotic host without one), `zap_accel_available()`
 * is 0 and the `fast` tier degrades to `native`.
 *
 * The tier/thread KNOBS below are pure getenv parsing and are ALWAYS compiled,
 * so the dispatcher reads them even in a no-provider build; only the provider
 * thread-pool call in `zap_accel_ensure_threads` is gated behind ZAP_HAVE_BLAS.
 * Included near the end of zaprt.c, after `zap_maxprocs` / `zap_setenv_compat`
 * are defined (same translation unit, so no forward declarations are needed).
 *
 * Naming is provider-neutral and survives replacing BLAS with an own
 * implementation (the "never say never" rule): the tiers auto/native/fast are
 * used identically by `ZAP_ACCEL`, the global knob, and the future
 * `@kernel(...)` call-site. */

/* `ZAP_ACCEL` selects the kernel tier: `auto` (default) lets the language pick,
 * `native` forces the own floor, `fast` forces the accelerator (→ native if
 * none is linked). Returned as 0=auto, 1=native, 2=fast; unset or unrecognized
 * is `auto`. */
int64_t zap_accel_tier(void) {
    const char *s = getenv("ZAP_ACCEL");
    if (s == NULL || s[0] == '\0')
        return 0;
    if (strcmp(s, "native") == 0)
        return 1;
    if (strcmp(s, "fast") == 0)
        return 2;
    return 0; /* "auto" and anything unrecognized */
}

/* `ZAP_ACCEL_THREADS` overrides the accelerator pool size. Unset, non-numeric,
 * trailing junk, or <= 0 returns -1, meaning "use the container-aware MAXPROCS
 * budget" (#905) — never the provider's default (default-threaded BLAS in a
 * 1-core pod spikes p99, #904). */
int64_t zap_accel_threads_env(void) {
    const char *s = getenv("ZAP_ACCEL_THREADS");
    if (s == NULL || s[0] == '\0')
        return -1;
    char *end = NULL;
    long v = strtol(s, &end, 10);
    if (end == s || *end != '\0' || v <= 0)
        return -1;
    return (int64_t)v;
}

/* 1 iff a BLAS/LAPACK accelerator is compiled in (a provider was found at build
 * time). A compile-time constant the `auto`/`fast` dispatch (#904) consults. */
int64_t zap_accel_available(void) {
#if defined(ZAP_HAVE_BLAS)
    return 1;
#else
    return 0;
#endif
}

/* Which accelerated ops the linked provider offers, as a bitmask (bit 0 =
 * gemm/matmul; later bits for gesv/geev/FFT, #512). gemm is in both OpenBLAS and
 * Accelerate, so availability implies it. */
int64_t zap_accel_ops(void) {
#if defined(ZAP_HAVE_BLAS)
    return 1; /* ZAP_ACCEL_OP_GEMM */
#else
    return 0;
#endif
}

#if defined(ZAP_HAVE_BLAS) && defined(ZAP_BLAS_OPENBLAS)
/* OpenBLAS extension, declared here so the seam needs no cblas header (gemm and
 * its <cblas.h> include arrive with the dispatcher, #904). */
extern void openblas_set_num_threads(int);
#endif

/* Pin the provider's internal thread pool to Zap's CPU budget on first use —
 * `ZAP_ACCEL_THREADS` if set, else the container-aware MAXPROCS (#905) — so a
 * BLAS hand-off never oversubscribes a constrained pod. Idempotent and cheap to
 * call before each accelerated op; a no-op when no provider is compiled in. The
 * dispatcher (#904) calls this before its first gemm. */
void zap_accel_ensure_threads(void) {
#if defined(ZAP_HAVE_BLAS)
    static int done = 0;
    if (done)
        return;
    done = 1;
    int64_t n = zap_accel_threads_env();
    if (n < 1)
        n = (int64_t)zap_maxprocs();
    if (n < 1)
        n = 1;
#if defined(ZAP_BLAS_OPENBLAS)
    openblas_set_num_threads((int)n);
#elif defined(ZAP_BLAS_ACCELERATE)
    /* Accelerate has no runtime set-threads call; it reads VECLIB_MAXIMUM_THREADS
     * once, before the first vecLib call — so set it here, ahead of any gemm. */
    char buf[32];
    snprintf(buf, sizeof(buf), "%lld", (long long)n);
    zap_setenv_compat("VECLIB_MAXIMUM_THREADS", buf, 1);
#endif
#endif
}
