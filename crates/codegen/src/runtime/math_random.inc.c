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
