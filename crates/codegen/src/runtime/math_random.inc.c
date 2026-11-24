/* zap runtime fragment: math_random.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * stdlib: math + random (ticket #98, D-0056)
 * ---------------------------------------------------------------------------
 * `math` — thin libm wrappers (unconditionally compiled for both collectors).
 * `random` — a seedable xoshiro256** generator with SplitMix64 seed expansion,
 * autoseeded from OS entropy (getentropy / arc4random) on first use. A single
 * mutex guards the global state so concurrent coroutines never observe torn
 * state; like CPython's module-level generator, the state itself is shared.
 * ========================================================================== */

/* --- math: libm wrappers --------------------------------------------------- */
/* `math.pow` reuses `zap_powf` (declared with the scalar numeric helpers). */
double zap_math_sqrt(double x) { return sqrt(x); }
double zap_math_exp(double x) { return exp(x); }
double zap_math_log(double x) { return log(x); }
double zap_math_log2(double x) { return log2(x); }
double zap_math_log10(double x) { return log10(x); }
double zap_math_sin(double x) { return sin(x); }
double zap_math_cos(double x) { return cos(x); }
double zap_math_tan(double x) { return tan(x); }
double zap_math_asin(double x) { return asin(x); }
double zap_math_acos(double x) { return acos(x); }
double zap_math_atan(double x) { return atan(x); }
double zap_math_floor(double x) { return floor(x); }
double zap_math_ceil(double x) { return ceil(x); }
double zap_math_round(double x) { return round(x); } /* half away from zero */
double zap_math_trunc(double x) { return trunc(x); }
double zap_math_fabs(double x) { return fabs(x); }

/* --- Python built-in round() (#1381) ---------------------------------------
 * Distinct from math.round above: the built-in uses banker's rounding
 * (ties-to-even) via `rint` under the default FE_TONEAREST mode, and the
 * no-ndigits form returns an integer. These are the helpers the `round`
 * built-in lowers to (declared with the scalar numeric helpers in lib.rs). */
int64_t zap_round_i64(double x) {
    double r = rint(x); /* ties to even */
    /* int64 range is [-2^63, 2^63); 2^63 == 9223372036854775808.0 as a double.
     * NaN and +/-inf fail both bounds and raise. */
    if (!(r >= -9223372036854775808.0 && r < 9223372036854775808.0)) {
        static const char m[] =
            "round() result has no int64 value (NaN, infinity, or out of range)";
        zap_panic_typed(ZAP_EXC_ARITHMETIC_ERROR, zap_str_new(m, sizeof(m) - 1));
    }
    return (int64_t)r;
}
double zap_round_nd(double x, int64_t n) {
    if (x != x) return x;   /* NaN passes through */
    if (x == 0.0) return x; /* preserve signed zero */
    if (n >= 0) {
        if (n > 323) return x; /* beyond double precision: already exact */
        double p = pow(10.0, (double)n);
        double scaled = x * p;
        /* Overflow to infinity, or a magnitude so large it has no fractional
         * part, means the value is already integral at this scale. */
        if (scaled != scaled || scaled + 1.0 == scaled || scaled - 1.0 == scaled) {
            return x;
        }
        return rint(scaled) / p;
    }
    /* Negative ndigits: DIVIDE by the integer 10^(-n) and multiply back, rather
     * than multiply by the inexact fraction 10^n — so a clean power-of-ten
     * result (e.g. round(12345.0, -2) == 12300.0) is exact for |n| <= 22, where
     * 10^(-n) is representable exactly in f64. */
    if (n < -308) return x < 0 ? -0.0 : 0.0; /* rounds to zero */
    double p = pow(10.0, (double)(-n));       /* integer-valued */
    double scaled = x / p;
    return rint(scaled) * p;
}
int64_t zap_iround_nd(int64_t x, int64_t n) {
    if (n >= 0) return x; /* an integer has no digits past the point */
    int64_t k = -n;
    if (k >= 19) {
        /* 10^19 > INT64_MAX, so |x| (< 10^19) rounds to 0 — except |x| >=
         * 5*10^18 at k==19 would round to +/-10^19, which overflows int64. */
        if (k == 19 &&
            (x >= 5000000000000000000LL || x <= -5000000000000000000LL)) {
            static const char m[] = "round() result has no int64 value (out of range)";
            zap_panic_typed(ZAP_EXC_ARITHMETIC_ERROR, zap_str_new(m, sizeof(m) - 1));
        }
        return 0;
    }
    int64_t p = 1;
    for (int64_t i = 0; i < k; i++) p *= 10;
    int64_t q = x / p;       /* truncated toward zero */
    int64_t rem = x - q * p; /* sign of x, |rem| < p */
    int64_t ar = rem < 0 ? -rem : rem;
    int64_t half = p / 2; /* p is a power of ten >= 10, so even */
    if (ar > half || (ar == half && (q & 1))) {
        q += (x < 0) ? -1 : 1; /* half-to-even: step away when q is odd */
    }
    if (q > INT64_MAX / p || q < INT64_MIN / p) {
        static const char m[] = "round() result has no int64 value (out of range)";
        zap_panic_typed(ZAP_EXC_ARITHMETIC_ERROR, zap_str_new(m, sizeof(m) - 1));
    }
    return q * p;
}
double zap_math_atan2(double y, double x) { return atan2(y, x); }
double zap_math_hypot(double a, double b) { return hypot(a, b); }
double zap_math_fmod(double a, double b) { return fmod(a, b); }
/* math.fma(x, y, z): the explicit fused multiply-add (#946, D-0196) — one
 * rounding of x*y + z. C99 `fma` is correctly rounded and uses the hardware FMA
 * where the target has it, so the result is bit-identical across platforms and
 * independent of the implicit-contraction `--fp` policy. */
double zap_math_fma(double x, double y, double z) { return fma(x, y, z); }
int64_t zap_math_isnan(double x) { return isnan(x) ? 1 : 0; }
int64_t zap_math_isinf(double x) { return isinf(x) ? 1 : 0; }

/* --- math: #973 everyday scalar additions ---------------------------------
 * Hyperbolics / accurate variants / special functions are thin libm wrappers;
 * like the elementary set above, a domain error yields NaN/inf (libm) rather
 * than raising — consistent with `sqrt(-1.0)` etc. The integer-math group and
 * the list reductions *do* raise catchable errors (routed via `throwing_call`
 * in lowering): a ValueError for a domain violation, an ArithmeticError for a
 * result that overflows the 64-bit integer range. (ticket #973, D-0234.) */

/* Angles. Multiply by the exact 180/pi (and its reciprocal) rather than call
 * out to libm, so `degrees(radians(x))` round-trips and `degrees(pi)` is 180. */
double zap_math_degrees(double x) { return x * (180.0 / 3.14159265358979323846); }
double zap_math_radians(double x) { return x * (3.14159265358979323846 / 180.0); }

/* Hyperbolic. */
double zap_math_sinh(double x) { return sinh(x); }
double zap_math_cosh(double x) { return cosh(x); }
double zap_math_tanh(double x) { return tanh(x); }
double zap_math_asinh(double x) { return asinh(x); }
double zap_math_acosh(double x) { return acosh(x); }
double zap_math_atanh(double x) { return atanh(x); }

/* Accurate variants (small-argument-stable exp/log, base-2 exp, cube root). */
double zap_math_expm1(double x) { return expm1(x); }
double zap_math_log1p(double x) { return log1p(x); }
double zap_math_exp2(double x) { return exp2(x); }
double zap_math_cbrt(double x) { return cbrt(x); }

/* Special functions (#973): true gamma, ln|gamma|, error function + complement.
 * libm's `lgamma` returns ln of the absolute value of gamma (the `signgam`
 * global holds the sign, which `math.lgamma` does not expose, matching CPython). */
double zap_math_gamma(double x) { return tgamma(x); }
double zap_math_lgamma(double x) { return lgamma(x); }
double zap_math_erf(double x) { return erf(x); }
double zap_math_erfc(double x) { return erfc(x); }

/* Classification / sign / adjacency. */
int64_t zap_math_isfinite(double x) { return isfinite(x) ? 1 : 0; }
double zap_math_copysign(double x, double y) { return copysign(x, y); }
double zap_math_nextafter(double x, double y) { return nextafter(x, y); }
double zap_math_remainder(double x, double y) { return remainder(x, y); }

/* ldexp(x, e) = x * 2**e. The exponent is a Zap `int`; clamp to the C `int`
 * range so an out-of-range magnitude saturates to 0/inf (as libm already would
 * for any representable but extreme exponent) instead of invoking UB on the
 * narrowing conversion. */
double zap_math_ldexp(double x, int64_t e) {
    if (e > 2147483647LL) {
        e = 2147483647LL;
    } else if (e < -2147483648LL) {
        e = -2147483648LL;
    }
    return ldexp(x, (int)e);
}

/* modf(x) -> (frac, int_part); frexp(x) -> (mantissa, exponent). Each component
 * is a separate wrapper so lowering can build the tuple from two scalar calls
 * (the os.path_split pattern), with no out-parameter ABI. */
double zap_math_modf_frac(double x) {
    double ip;
    return modf(x, &ip);
}
double zap_math_modf_ipart(double x) {
    double ip;
    (void)modf(x, &ip);
    return ip;
}
double zap_math_frexp_mant(double x) {
    int e;
    return frexp(x, &e);
}
int64_t zap_math_frexp_exp(double x) {
    int e;
    (void)frexp(x, &e);
    return (int64_t)e;
}

/* isclose(a, b, rel_tol, abs_tol) — CPython `math.isclose` semantics:
 * |a-b| <= max(rel_tol * max(|a|, |b|), abs_tol). Identical values (incl. two
 * equal infinities) are always close; a lone infinity never is; a NaN operand
 * is never close (the `<=` is false). A negative tolerance raises ValueError. */
int64_t zap_math_isclose(double a, double b, double rel_tol, double abs_tol) {
    if (rel_tol < 0.0 || abs_tol < 0.0) {
        const char *m = "math.isclose(): tolerances must be non-negative";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    if (a == b) {
        return 1; /* exact equality, including +inf==+inf and -inf==-inf */
    }
    if (isinf(a) || isinf(b)) {
        return 0; /* a != b with an infinity involved is never close */
    }
    double diff = fabs(b - a);
    double tol = rel_tol * fmax(fabs(a), fabs(b));
    if (tol < abs_tol) {
        tol = abs_tol;
    }
    return (diff <= tol) ? 1 : 0; /* a NaN operand makes diff NaN -> false -> 0 */
}

/* Integer-math helpers. gcd over unsigned words (INT64_MIN-safe magnitudes). */
static uint64_t zap_math_abs_u64(int64_t v) {
    return (v < 0) ? (~(uint64_t)v + 1ULL) : (uint64_t)v; /* two's-complement |v| */
}
static uint64_t zap_math_gcd_u64(uint64_t a, uint64_t b) {
    while (b != 0ULL) {
        uint64_t t = a % b;
        a = b;
        b = t;
    }
    return a;
}

/* Raise the shared "result does not fit in a 64-bit integer" ArithmeticError. */
__attribute__((noreturn)) static void zap_math_int_overflow(const char *who) {
    char buf[96];
    int n = snprintf(buf, sizeof buf, "%s: result exceeds the 64-bit integer range", who);
    if (n < 0) {
        n = 0;
    }
    zap_panic_typed(ZAP_EXC_ARITHMETIC_ERROR, zap_str_new(buf, (int64_t)n));
    abort(); /* unreachable: zap_panic_typed unwinds */
}

/* gcd(a, b): greatest common divisor of the magnitudes, always >= 0 (Python
 * `math.gcd`). gcd(0, 0) == 0. Total — never raises. (The sole unrepresentable
 * result, gcd(INT64_MIN, 0) == 2**63, wraps to INT64_MIN; documented.) */
int64_t zap_math_gcd(int64_t a, int64_t b) {
    return (int64_t)zap_math_gcd_u64(zap_math_abs_u64(a), zap_math_abs_u64(b));
}

/* lcm(a, b): least common multiple of the magnitudes, always >= 0. lcm with a
 * zero operand is 0 (Python). Raises ArithmeticError if the result overflows. */
int64_t zap_math_lcm(int64_t a, int64_t b) {
    if (a == 0 || b == 0) {
        return 0;
    }
    uint64_t x = zap_math_abs_u64(a);
    uint64_t y = zap_math_abs_u64(b);
    uint64_t q = x / zap_math_gcd_u64(x, y); /* x / gcd, exact */
    if (q > (uint64_t)INT64_MAX / y) {
        zap_math_int_overflow("math.lcm()");
    }
    return (int64_t)(q * y);
}

/* factorial(n): n! for n >= 0. n < 0 raises ValueError; n > 20 overflows the
 * 64-bit range (21! > INT64_MAX) and raises ArithmeticError. */
int64_t zap_math_factorial(int64_t n) {
    if (n < 0) {
        const char *m = "math.factorial(): argument must be non-negative";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    if (n > 20) {
        zap_math_int_overflow("math.factorial()");
    }
    int64_t r = 1;
    for (int64_t i = 2; i <= n; i++) {
        r *= i; /* exact: 20! fits */
    }
    return r;
}

/* comb(n, k): binomial coefficient "n choose k". n, k >= 0 (else ValueError);
 * k > n yields 0. Exact and overflow-checked in pure uint64: each step keeps
 * `r == C(n, i+1)` by multiplying by (n-i) and dividing by (i+1), reducing the
 * two by their gcd first so `r` never exceeds the running coefficient (hence no
 * spurious overflow when the final value fits). ArithmeticError on true
 * overflow. */
int64_t zap_math_comb(int64_t n, int64_t k) {
    if (n < 0 || k < 0) {
        const char *m = "math.comb(): n and k must be non-negative";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    if (k > n) {
        return 0;
    }
    if (k > n - k) {
        k = n - k; /* symmetry: C(n, k) == C(n, n-k); minimize the loop */
    }
    uint64_t r = 1ULL;
    for (int64_t i = 0; i < k; i++) {
        uint64_t a = (uint64_t)(n - i); /* n-i > 0 for i < k <= n */
        uint64_t b = (uint64_t)(i + 1);
        uint64_t g = zap_math_gcd_u64(a, b);
        a /= g;
        b /= g;
        r /= b; /* exact: with gcd(a,b)==1 and b | r*a, b divides r */
        if (a != 0ULL && r > (uint64_t)INT64_MAX / a) {
            zap_math_int_overflow("math.comb()");
        }
        r *= a; /* r == C(n, i+1) <= final result, so this never overflows u64 */
    }
    return (int64_t)r;
}

/* perm(n, k): falling factorial n*(n-1)*...*(n-k+1) = n! / (n-k)!. n, k >= 0
 * (else ValueError); k > n yields 0. Overflow-checked -> ArithmeticError. */
int64_t zap_math_perm(int64_t n, int64_t k) {
    if (n < 0 || k < 0) {
        const char *m = "math.perm(): n and k must be non-negative";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    if (k > n) {
        return 0;
    }
    uint64_t r = 1ULL;
    for (int64_t i = 0; i < k; i++) {
        uint64_t a = (uint64_t)(n - i);
        if (a != 0ULL && r > (uint64_t)INT64_MAX / a) {
            zap_math_int_overflow("math.perm()");
        }
        r *= a;
    }
    return (int64_t)r;
}

/* isqrt(n): floor(sqrt(n)) for n >= 0 (n < 0 raises ValueError). The double
 * seed is corrected by at most a couple of integer steps, so the result is
 * exact across the whole 64-bit range (all divisions avoid r*r overflow). */
int64_t zap_math_isqrt(int64_t n) {
    if (n < 0) {
        const char *m = "math.isqrt(): argument must be non-negative";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    uint64_t x = (uint64_t)n;
    if (x == 0ULL) {
        return 0;
    }
    uint64_t r = (uint64_t)sqrt((double)x);
    if (r == 0ULL) {
        r = 1ULL; /* guard the x/r divisions below */
    }
    while (r > x / r) {
        r--; /* r*r > x: shrink until r*r <= x (division avoids overflow) */
    }
    while ((r + 1ULL) <= x / (r + 1ULL)) {
        r++; /* (r+1)^2 <= x: grow to the true floor */
    }
    return (int64_t)r;
}

/* List reductions over a ZapList[T] (element word count = list->len). The int
 * forms wrap on overflow, matching the `*`/`+` operators on `int`; the float
 * forms read each word back as a double. */
int64_t zap_math_prod_i64(void *l) {
    ZapList *list = (ZapList *)l;
    int64_t r = 1;
    for (int64_t i = 0; i < list->len; i++) {
        r *= list->data[i]; /* two's-complement wrap, as for `int` `*` */
    }
    return r;
}
double zap_math_prod_f64(void *l) {
    ZapList *list = (ZapList *)l;
    double r = 1.0;
    for (int64_t i = 0; i < list->len; i++) {
        r *= zap_f64_from_bits(list->data[i]);
    }
    return r;
}

/* fsum(xs): Shewchuk's exact floating-point summation (CPython `math.fsum`).
 * Maintains a set of non-overlapping partial sums whose exact total equals the
 * running sum; the final add of the partials rounds once. Defeats the rounding
 * error a naive left-to-right sum accumulates (e.g. [0.1]*10 -> exactly 1.0). */
double zap_math_fsum(void *l) {
    ZapList *list = (ZapList *)l;
    double stackbuf[64];
    double *partials = stackbuf;
    int cap = 64;
    int np = 0;
    int on_heap = 0;
    for (int64_t idx = 0; idx < list->len; idx++) {
        double x = zap_f64_from_bits(list->data[idx]);
        int i = 0;
        for (int j = 0; j < np; j++) {
            double y = partials[j];
            if (fabs(x) < fabs(y)) {
                double t = x;
                x = y;
                y = t;
            }
            double hi = x + y;
            double lo = y - (hi - x); /* exact round-off of x + y */
            if (lo != 0.0) {
                partials[i++] = lo;
            }
            x = hi;
        }
        np = i;
        if (np + 1 > cap) {
            int ncap = cap * 2;
            double *nb = (double *)malloc((size_t)ncap * sizeof(double));
            if (nb == NULL) {
                if (on_heap) {
                    free(partials);
                }
                const char *m = "math.fsum(): out of memory";
                zap_panic_typed(ZAP_EXC_RUNTIME_ERROR, zap_str_new(m, (int64_t)strlen(m)));
            }
            memcpy(nb, partials, (size_t)np * sizeof(double));
            if (on_heap) {
                free(partials);
            }
            partials = nb;
            cap = ncap;
            on_heap = 1;
        }
        partials[np++] = x;
    }
    double total = 0.0;
    for (int j = 0; j < np; j++) {
        total += partials[j];
    }
    if (on_heap) {
        free(partials);
    }
    return total;
}

/* dist(p, q): Euclidean distance sqrt(sum((p_i - q_i)**2)) over two equal-length
 * float lists. A length mismatch raises ValueError (Python). */
double zap_math_dist(void *pp, void *qq) {
    ZapList *p = (ZapList *)pp;
    ZapList *q = (ZapList *)qq;
    if (p->len != q->len) {
        const char *m = "math.dist(): both points must have the same number of dimensions";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    double s = 0.0;
    for (int64_t i = 0; i < p->len; i++) {
        double d = zap_f64_from_bits(p->data[i]) - zap_f64_from_bits(q->data[i]);
        s += d * d;
    }
    return sqrt(s);
}

/* sumprod(p, q): sum of the element-wise products of two equal-length lists.
 * A length mismatch raises ValueError. The int form wraps on overflow. */
int64_t zap_math_sumprod_i64(void *pp, void *qq) {
    ZapList *p = (ZapList *)pp;
    ZapList *q = (ZapList *)qq;
    if (p->len != q->len) {
        const char *m = "math.sumprod(): the sequences must have the same length";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    int64_t s = 0;
    for (int64_t i = 0; i < p->len; i++) {
        s += p->data[i] * q->data[i]; /* two's-complement wrap */
    }
    return s;
}
double zap_math_sumprod_f64(void *pp, void *qq) {
    ZapList *p = (ZapList *)pp;
    ZapList *q = (ZapList *)qq;
    if (p->len != q->len) {
        const char *m = "math.sumprod(): the sequences must have the same length";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    double s = 0.0;
    for (int64_t i = 0; i < p->len; i++) {
        s += zap_f64_from_bits(p->data[i]) * zap_f64_from_bits(q->data[i]);
    }
    return s;
}

/* --- random: xoshiro256** -------------------------------------------------- */
static uint64_t zap_rng_s[4];
static int zap_rng_seeded = 0;
static pthread_mutex_t zap_rng_mu = PTHREAD_MUTEX_INITIALIZER;

/* SplitMix64: expands a single 64-bit seed into the four xoshiro words. */
static uint64_t zap_splitmix64(uint64_t *x) {
    uint64_t z = (*x += 0x9e3779b97f4a7c15ULL);
    z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
    z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
    return z ^ (z >> 31);
}

/* One 64-bit word of OS entropy (never all-zero). */
static uint64_t zap_os_entropy_u64(void) {
    uint64_t s = 0;
#if defined(__APPLE__)
    arc4random_buf(&s, sizeof s);
#elif defined(__linux__)
    if (getentropy(&s, sizeof s) != 0) {
        s = (uint64_t)time(NULL) ^ ((uint64_t)clock() << 21);
    }
#elif defined(ZAP_PLATFORM_WINDOWS)
    if (BCryptGenRandom(NULL, (PUCHAR)&s, (ULONG)sizeof s,
                        BCRYPT_USE_SYSTEM_PREFERRED_RNG) != 0) {
        s = (uint64_t)time(NULL) ^ ((uint64_t)clock() << 21);
    }
#else
    s = (uint64_t)time(NULL) ^ ((uint64_t)clock() << 21);
#endif
    if (s == 0) {
        s = 0x9e3779b97f4a7c15ULL;
    }
    return s;
}

/* Reseed from a 64-bit value. Caller holds `zap_rng_mu`. */
static void zap_rng_seed_locked(uint64_t seed) {
    uint64_t sm = seed;
    for (int i = 0; i < 4; i++) {
        zap_rng_s[i] = zap_splitmix64(&sm);
    }
    zap_rng_seeded = 1;
}

/* Autoseed from OS entropy on first use. Caller holds `zap_rng_mu`. */
static void zap_rng_ensure_locked(void) {
    if (!zap_rng_seeded) {
        zap_rng_seed_locked(zap_os_entropy_u64());
    }
}

static inline uint64_t zap_rotl(uint64_t x, int k) {
    return (x << k) | (x >> (64 - k));
}

/* xoshiro256** step. Caller holds `zap_rng_mu` and has ensured seeding. */
static uint64_t zap_rng_next_locked(void) {
    uint64_t *s = zap_rng_s;
    const uint64_t result = zap_rotl(s[1] * 5, 7) * 9;
    const uint64_t t = s[1] << 17;
    s[2] ^= s[0];
    s[3] ^= s[1];
    s[1] ^= s[2];
    s[0] ^= s[3];
    s[2] ^= t;
    s[3] = zap_rotl(s[3], 45);
    return result;
}

/* Unbiased integer in [0, n) via rejection (n >= 1). Caller holds the lock. */
static uint64_t zap_rng_bounded_locked(uint64_t n) {
    uint64_t threshold = (0ULL - n) % n;
    uint64_t r;
    do {
        r = zap_rng_next_locked();
    } while (r < threshold);
    return r % n;
}

void zap_random_seed(int64_t n) {
    zap_np_lock(&zap_rng_mu);
    zap_rng_seed_locked((uint64_t)n);
    zap_np_unlock(&zap_rng_mu);
}

/* A double in [0, 1): the top 53 bits scaled by 2^-53. */
double zap_random_f64(void) {
    zap_np_lock(&zap_rng_mu);
    zap_rng_ensure_locked();
    uint64_t x = zap_rng_next_locked();
    zap_np_unlock(&zap_rng_mu);
    return (double)(x >> 11) * (1.0 / 9007199254740992.0); /* 2^53 */
}

double zap_random_uniform(double a, double b) {
    return a + (b - a) * zap_random_f64();
}

int64_t zap_random_randint(int64_t a, int64_t b) {
    if (b < a) {
        fprintf(stderr, "zap: random.randint(a, b) requires a <= b\n");
        abort();
    }
    uint64_t span = (uint64_t)(b - a) + 1ULL; /* inclusive, wrap-safe */
    zap_np_lock(&zap_rng_mu);
    zap_rng_ensure_locked();
    uint64_t off = (span == 0ULL) /* a..b spans the full int64 width */
                       ? zap_rng_next_locked()
                       : zap_rng_bounded_locked(span);
    zap_np_unlock(&zap_rng_mu);
    return (int64_t)((uint64_t)a + off);
}

/* A random valid index into a non-empty list (backs `random.choice`). */
int64_t zap_random_list_index(void *l) {
    ZapList *list = (ZapList *)l;
    if (list->len <= 0) {
        fprintf(stderr, "zap: random.choice from an empty sequence\n");
        abort();
    }
    zap_np_lock(&zap_rng_mu);
    zap_rng_ensure_locked();
    uint64_t idx = zap_rng_bounded_locked((uint64_t)list->len);
    zap_np_unlock(&zap_rng_mu);
    return (int64_t)idx;
}

/* Fisher-Yates in place over the element words (works for any element kind;
 * pointer words keep their GC identity, only their positions change). */
void zap_random_shuffle(void *l) {
    ZapList *list = (ZapList *)l;
    zap_np_lock(&zap_rng_mu);
    zap_rng_ensure_locked();
    for (int64_t i = list->len - 1; i > 0; i--) {
        uint64_t j = zap_rng_bounded_locked((uint64_t)(i + 1));
        int64_t tmp = list->data[i];
        list->data[i] = list->data[(int64_t)j];
        list->data[(int64_t)j] = tmp;
    }
    zap_np_unlock(&zap_rng_mu);
}

