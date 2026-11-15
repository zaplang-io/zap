/* ===========================================================================
 * random — NumPy-compatible bit-exact PCG64 Generator (#930/#931, D-0189)
 *
 * A seedable PCG64 (XSL-RR 128/64) bit generator with NumPy's SeedSequence
 * entropy expansion, and the NumPy `Generator` distribution/sampling surface,
 * reproducing `numpy.random.default_rng(seed).<method>(...)` VALUE-FOR-VALUE.
 * The engine (SeedSequence + PCG64 + Lemire-bounded integers + the ziggurat /
 * rejection distribution algorithms + choice/shuffle) follows NumPy's
 * `bit_generator.pyx` / `pcg64.*` / `distributions.c` / `_generator.pyx`
 * (BSD-3) exactly; the ziggurat tables are vendored verbatim in
 * `ziggurat_constants.h` (beside this file). Every path here was cross-checked
 * bit-for-bit against NumPy 2.5.4 before landing.
 *
 * This is the array `Generator` surface and coexists with the pre-existing
 * module-level xoshiro256** `random` (D-0056): a `Generator` handle is an
 * opaque GC-allocated, pointer-free leaf (`ZapRng`), threaded as the first
 * argument of every call. Not safe to share across coroutines concurrently
 * (matching NumPy's Generator, which is single-stream).
 *
 * Included by `zaprt.c` AFTER the ZapArray helpers and the xoshiro block, so
 * the array constructors/accessors and `zap_os_entropy_u64` are in scope.
 * ======================================================================== */

#if !defined(__SIZEOF_INT128__)
#error "zap random Generator requires a 128-bit integer type (__int128)"
#endif

#include "ziggurat_constants.h"

typedef __uint128_t zap_u128;

/* ---- NumPy SeedSequence (bit_generator.pyx) ---- */
#define ZAP_SS_INIT_A 0x43b0d7e5u
#define ZAP_SS_MULT_A 0x931e8875u
#define ZAP_SS_INIT_B 0x8b51f9ddu
#define ZAP_SS_MULT_B 0x58f38dedu
#define ZAP_SS_MIX_L 0xca01f9ddu
#define ZAP_SS_MIX_R 0x4973f715u
#define ZAP_SS_XSHIFT 16u
#define ZAP_SS_POOL 4

static uint32_t zap_ss_hashmix(uint32_t value, uint32_t *hash_const) {
    value ^= hash_const[0];
    hash_const[0] *= ZAP_SS_MULT_A;
    value *= hash_const[0];
    value ^= value >> ZAP_SS_XSHIFT;
    return value;
}
static uint32_t zap_ss_mix(uint32_t x, uint32_t y) {
    uint32_t r = (ZAP_SS_MIX_L * x) - (ZAP_SS_MIX_R * y);
    r ^= r >> ZAP_SS_XSHIFT;
    return r;
}
/* SeedSequence(seed).generate_state(4, uint64) for a nonnegative integer seed.
 * entropy = little-endian uint32 words of the seed (>= 1 word); pool mixed per
 * NumPy, then 8 uint32 generated and viewed as 4 little-endian uint64. */
static void zap_ss_state4(uint64_t seed, uint64_t out[4]) {
    uint32_t ent[4];
    int elen = 0;
    ent[elen++] = (uint32_t)(seed & 0xffffffffu);
    uint64_t rest = seed >> 32;
    while (rest) {
        ent[elen++] = (uint32_t)(rest & 0xffffffffu);
        rest >>= 32;
    }
    uint32_t mixer[ZAP_SS_POOL];
    uint32_t hc[1] = {ZAP_SS_INIT_A};
    for (int i = 0; i < ZAP_SS_POOL; i++)
        mixer[i] = zap_ss_hashmix(i < elen ? ent[i] : 0u, hc);
    for (int is = 0; is < ZAP_SS_POOL; is++)
        for (int id = 0; id < ZAP_SS_POOL; id++)
            if (is != id)
                mixer[id] = zap_ss_mix(mixer[id], zap_ss_hashmix(mixer[is], hc));
    for (int is = ZAP_SS_POOL; is < elen; is++)
        for (int id = 0; id < ZAP_SS_POOL; id++)
            mixer[id] = zap_ss_mix(mixer[id], zap_ss_hashmix(ent[is], hc));
    uint32_t hb = ZAP_SS_INIT_B, s32[8];
    for (int i = 0; i < 8; i++) {
        uint32_t dv = mixer[i % ZAP_SS_POOL];
        dv ^= hb;
        hb *= ZAP_SS_MULT_B;
        dv *= hb;
        dv ^= dv >> ZAP_SS_XSHIFT;
        s32[i] = dv;
    }
    for (int i = 0; i < 4; i++)
        out[i] = (uint64_t)s32[2 * i] | ((uint64_t)s32[2 * i + 1] << 32);
}

/* ---- PCG64 (XSL-RR 128/64), NumPy's default_rng bit generator ---- */
#define ZAP_PCG_MULT (((zap_u128)0x2360ed051fc65da4ULL << 64) | 0x4385df649fccf645ULL)

typedef struct {
    zap_u128 state;
    zap_u128 inc;
    int has_uint32;    /* 32-bit draw buffer (shared with 64-bit draws) */
    uint32_t uinteger;
} ZapRng;

static void zap_pcg_step(ZapRng *r) { r->state = r->state * ZAP_PCG_MULT + r->inc; }
static uint64_t zap_pcg_output(zap_u128 s) {
    uint64_t hi = (uint64_t)(s >> 64), lo = (uint64_t)s, x = hi ^ lo;
    unsigned rot = (unsigned)(s >> 122);
    return (x >> rot) | (x << ((-rot) & 63));
}
/* next_uint64: advance then output. Does NOT touch the 32-bit buffer. */
static uint64_t zap_rng_next64(ZapRng *r) {
    zap_pcg_step(r);
    return zap_pcg_output(r->state);
}
/* next_uint32: low 32 bits of a fresh 64-bit draw first, then the high 32. The
 * buffer lives in the generator state (persists across calls, bypassed by
 * 64-bit draws) — NumPy's pcg64_next32. */
static uint32_t zap_rng_next32(ZapRng *r) {
    if (r->has_uint32) {
        r->has_uint32 = 0;
        return r->uinteger;
    }
    uint64_t v = zap_rng_next64(r);
    r->has_uint32 = 1;
    r->uinteger = (uint32_t)(v >> 32);
    return (uint32_t)(v & 0xffffffffu);
}
/* A double in [0, 1): top 53 bits scaled by 2^-53 (NumPy next_double). */
static double zap_rng_nextd(ZapRng *r) {
    return (zap_rng_next64(r) >> 11) * (1.0 / 9007199254740992.0);
}
static void zap_rng_srandom(ZapRng *r, zap_u128 initstate, zap_u128 initseq) {
    r->state = 0;
    r->inc = (initseq << 1) | 1;
    zap_pcg_step(r);
    r->state += initstate;
    zap_pcg_step(r);
    r->has_uint32 = 0;
    r->uinteger = 0;
}
static void zap_rng_seed_int(ZapRng *r, uint64_t seed) {
    uint64_t w[4];
    zap_ss_state4(seed, w);
    zap_u128 initstate = ((zap_u128)w[0] << 64) | w[1];
    zap_u128 initseq = ((zap_u128)w[2] << 64) | w[3];
    zap_rng_srandom(r, initstate, initseq);
}

/* ---- public: construct a Generator handle ---- */
void *zap_random_default_rng(int64_t seed) {
    ZapRng *r = (ZapRng *)zap_buf(sizeof(ZapRng)); /* pointer-free leaf */
    zap_rng_seed_int(r, (uint64_t)seed);
    return r;
}
void *zap_random_default_rng_entropy(void) {
    ZapRng *r = (ZapRng *)zap_buf(sizeof(ZapRng));
    zap_rng_seed_int(r, zap_os_entropy_u64());
    return r;
}

/* ---- bounded integers (NumPy Lemire, use_masked = 0) ---- */
static uint64_t zap_gen_mask(uint64_t m) {
    m |= m >> 1; m |= m >> 2; m |= m >> 4; m |= m >> 8; m |= m >> 16; m |= m >> 32;
    return m;
}
static uint32_t zap_lemire32(ZapRng *r, uint32_t rng) {
    uint32_t rng_excl = rng + 1;
    uint64_t m = (uint64_t)zap_rng_next32(r) * rng_excl;
    uint32_t leftover = (uint32_t)m;
    if (leftover < rng_excl) {
        uint32_t threshold = (uint32_t)(-rng_excl) % rng_excl;
        while (leftover < threshold) {
            m = (uint64_t)zap_rng_next32(r) * rng_excl;
            leftover = (uint32_t)m;
        }
    }
    return (uint32_t)(m >> 32);
}
static uint64_t zap_lemire64(ZapRng *r, uint64_t rng) {
    uint64_t rng_excl = rng + 1;
    zap_u128 m = (zap_u128)zap_rng_next64(r) * rng_excl;
    uint64_t leftover = (uint64_t)m;
    if (leftover < rng_excl) {
        uint64_t threshold = (uint64_t)(-rng_excl) % rng_excl;
        while (leftover < threshold) {
            m = (zap_u128)zap_rng_next64(r) * rng_excl;
            leftover = (uint64_t)m;
        }
    }
    return (uint64_t)(m >> 64);
}
/* scalar bounded [off, off+rng] inclusive (NumPy random_bounded_uint64). */
static uint64_t zap_rng_bounded(ZapRng *r, uint64_t off, uint64_t rng) {
    if (rng == 0) return off;
    if (rng <= 0xFFFFFFFFUL) {
        if (rng == 0xFFFFFFFFUL) return off + (uint64_t)zap_rng_next32(r);
        return off + zap_lemire32(r, (uint32_t)rng);
    }
    if (rng == 0xFFFFFFFFFFFFFFFFULL) return off + zap_rng_next64(r);
    return off + zap_lemire64(r, rng);
}
/* masked rejection in [0, max] (NumPy random_interval; used by shuffle). */
static uint64_t zap_rng_interval(ZapRng *r, uint64_t max) {
    if (max == 0) return 0;
    uint64_t mask = zap_gen_mask(max), v;
    if (max <= 0xffffffffUL) {
        while ((v = (zap_rng_next32(r) & mask)) > max) {}
    } else {
        while ((v = (zap_rng_next64(r) & mask)) > max) {}
    }
    return v;
}
/* one integer in [low, high) (endpoint == 0) or [low, high] (endpoint != 0). */
static int64_t zap_gen_integer(ZapRng *r, int64_t low, int64_t high, int64_t endpoint) {
    if (endpoint == 0) high -= 1;
    uint64_t rng = (uint64_t)(high - low), off = (uint64_t)low;
    return (int64_t)zap_rng_bounded(r, off, rng);
}

/* ---- continuous distributions (NumPy distributions.c, ported verbatim) ---- */
static double zap_gen_std_normal(ZapRng *r) {
    uint64_t rr;
    int sign, idx;
    uint64_t rabs;
    double x, xx, yy;
    for (;;) {
        rr = zap_rng_next64(r);
        idx = rr & 0xff;
        rr >>= 8;
        sign = rr & 0x1;
        rabs = (rr >> 1) & 0x000fffffffffffffULL;
        x = rabs * wi_double[idx];
        if (sign & 0x1) x = -x;
        if (rabs < ki_double[idx]) return x;
        if (idx == 0) {
            for (;;) {
                xx = -ziggurat_nor_inv_r * log1p(-zap_rng_nextd(r));
                yy = -log1p(-zap_rng_nextd(r));
                if (yy + yy > xx * xx)
                    return ((rabs >> 8) & 0x1) ? -(ziggurat_nor_r + xx)
                                               : ziggurat_nor_r + xx;
            }
        } else {
            if (((fi_double[idx - 1] - fi_double[idx]) * zap_rng_nextd(r) +
                 fi_double[idx]) < exp(-0.5 * x * x))
                return x;
        }
    }
}
static double zap_gen_std_exp(ZapRng *r) {
    uint64_t ri = zap_rng_next64(r);
    ri >>= 3;
    uint8_t idx = ri & 0xFF;
    ri >>= 8;
    double x = ri * we_double[idx];
    if (ri < ke_double[idx]) return x;
    for (;;) { /* standard_exponential_unlikely, recursion flattened */
        if (idx == 0) return ziggurat_exp_r - log1p(-zap_rng_nextd(r));
        if ((fe_double[idx - 1] - fe_double[idx]) * zap_rng_nextd(r) + fe_double[idx] <
            exp(-x))
            return x;
        ri = zap_rng_next64(r);
        ri >>= 3;
        idx = ri & 0xFF;
        ri >>= 8;
        x = ri * we_double[idx];
        if (ri < ke_double[idx]) return x;
    }
}
static double zap_gen_std_gamma(ZapRng *r, double shape) {
    double b, c, U, V, X, Y;
    if (shape == 1.0) return zap_gen_std_exp(r);
    if (shape == 0.0) return 0.0;
    if (shape < 1.0) {
        for (;;) {
            U = zap_rng_nextd(r);
            V = zap_gen_std_exp(r);
            if (U <= 1.0 - shape) {
                X = pow(U, 1. / shape);
                if (X <= V) return X;
            } else {
                Y = -log((1 - U) / shape);
                X = pow(1.0 - shape + shape * Y, 1. / shape);
                if (X <= (V + Y)) return X;
            }
        }
    }
    b = shape - 1. / 3.;
    c = 1. / sqrt(9 * b);
    for (;;) {
        do {
            X = zap_gen_std_normal(r);
            V = 1.0 + c * X;
        } while (V <= 0.0);
        V = V * V * V;
        U = zap_rng_nextd(r);
        if (U < 1.0 - 0.0331 * (X * X) * (X * X)) return b * V;
        if (log(U) < 0.5 * X * X + b * (1. - V + log(V))) return b * V;
    }
}
#define ZAP_BETA_TINY 3e-103
static double zap_gen_beta(ZapRng *r, double a, double b) {
    double Ga, Gb;
    if (a <= 1.0 && b <= 1.0) {
        double U, V, X, Y, XpY;
        if (a < ZAP_BETA_TINY && b < ZAP_BETA_TINY) {
            U = zap_rng_nextd(r);
            return (a + b) * U < a;
        }
        while (1) {
            U = zap_rng_nextd(r);
            V = zap_rng_nextd(r);
            X = pow(U, 1.0 / a);
            Y = pow(V, 1.0 / b);
            XpY = X + Y;
            if (XpY <= 1.0 && U + V > 0.0) {
                if (X > 0 && Y > 0) return X / XpY;
                double lX = log(U) / a, lY = log(V) / b, d = lX - lY;
                if (d > 0) return exp(-log1p(exp(-d)));
                return exp(d - log1p(exp(d)));
            }
        }
    }
    Ga = zap_gen_std_gamma(r, a);
    Gb = zap_gen_std_gamma(r, b);
    return Ga / (Ga + Gb);
}
static double zap_gen_loggam(double x) {
    double x0, x2, lg2pi, gl, gl0;
    long k, n;
    static const double a[10] = {
        8.333333333333333e-02,  -2.777777777777778e-03, 7.936507936507937e-04,
        -5.952380952380952e-04, 8.417508417508418e-04,  -1.917526917526918e-03,
        6.410256410256410e-03,  -2.955065359477124e-02, 1.796443723688307e-01,
        -1.39243221690590e+00};
    if (x == 1.0 || x == 2.0) return 0.0;
    else if (x < 7.0) n = (long)(7 - x);
    else n = 0;
    x0 = x + n;
    x2 = (1.0 / x0) * (1.0 / x0);
    lg2pi = 1.8378770664093453e+00;
    gl0 = a[9];
    for (k = 8; k >= 0; k--) {
        gl0 *= x2;
        gl0 += a[k];
    }
    gl = gl0 / x0 + 0.5 * lg2pi + (x0 - 0.5) * log(x0) - x0;
    if (x < 7.0) {
        for (k = 1; k <= n; k++) {
            gl -= log(x0 - 1.0);
            x0 -= 1.0;
        }
    }
    return gl;
}
static int64_t zap_gen_poisson_mult(ZapRng *r, double lam) {
    double enlam = exp(-lam), prod = 1.0, U;
    int64_t X = 0;
    while (1) {
        U = zap_rng_nextd(r);
        prod *= U;
        if (prod > enlam) X += 1;
        else return X;
    }
}
static int64_t zap_gen_poisson_ptrs(ZapRng *r, double lam) {
    int64_t k;
    double U, V, slam = sqrt(lam), loglam = log(lam), a, b, invalpha, vr, us;
    b = 0.931 + 2.53 * slam;
    a = -0.059 + 0.02483 * b;
    invalpha = 1.1239 + 1.1328 / (b - 3.4);
    vr = 0.9277 - 3.6224 / (b - 2);
    while (1) {
        U = zap_rng_nextd(r) - 0.5;
        V = zap_rng_nextd(r);
        us = 0.5 - fabs(U);
        k = (int64_t)floor((2 * a / us + b) * U + lam + 0.43);
        if (us >= 0.07 && V <= vr) return k;
        if (k < 0 || (us < 0.013 && V > us)) continue;
        if ((log(V) + log(invalpha) - log(a / (us * us) + b)) <=
            (-lam + (double)k * loglam - zap_gen_loggam((double)k + 1)))
            return k;
    }
}
static int64_t zap_gen_poisson(ZapRng *r, double lam) {
    if (lam >= 10) return zap_gen_poisson_ptrs(r, lam);
    if (lam == 0) return 0;
    return zap_gen_poisson_mult(r, lam);
}
static int64_t zap_gen_binom_inv(ZapRng *r, int64_t n, double p) {
    double q = 1.0 - p, qn = exp(n * log1p(-p)), np = n * p, px, U;
    int64_t X, bound = (int64_t)fmin((double)n, np + 10.0 * sqrt(np * q + 1));
    X = 0;
    px = qn;
    U = zap_rng_nextd(r);
    while (U > px) {
        X++;
        if (X > bound) {
            X = 0;
            px = qn;
            U = zap_rng_nextd(r);
        } else {
            U -= px;
            px = ((n - X + 1) * p * px) / (X * q);
        }
    }
    return X;
}
static int64_t zap_gen_binom_btpe(ZapRng *r, int64_t n, double p) {
    double rr, q, fm, p1, xm, xl, xr, c, laml, lamr, p2, p3, p4;
    double a, u, v, s, F, rho, t, A, nrq, x1, x2, f1, f2, z, z2, w, w2, x;
    int64_t m, y, k, i;
    rr = fmin(p, 1.0 - p);
    q = 1.0 - rr;
    fm = n * rr + rr;
    m = (int64_t)floor(fm);
    p1 = floor(2.195 * sqrt(n * rr * q) - 4.6 * q) + 0.5;
    xm = m + 0.5;
    xl = xm - p1;
    xr = xm + p1;
    c = 0.134 + 20.5 / (15.3 + m);
    a = (fm - xl) / (fm - xl * rr);
    laml = a * (1.0 + a / 2.0);
    a = (xr - fm) / (xr * q);
    lamr = a * (1.0 + a / 2.0);
    p2 = p1 * (1.0 + 2.0 * c);
    p3 = p2 + c / laml;
    p4 = p3 + c / lamr;
Step10:
    nrq = n * rr * q;
    u = zap_rng_nextd(r) * p4;
    v = zap_rng_nextd(r);
    if (u > p1) goto Step20;
    y = (int64_t)floor(xm - p1 * v + u);
    goto Step60;
Step20:
    if (u > p2) goto Step30;
    x = xl + (u - p1) / c;
    v = v * c + 1.0 - fabs(m - x + 0.5) / p1;
    if (v > 1.0) goto Step10;
    y = (int64_t)floor(x);
    goto Step50;
Step30:
    if (u > p3) goto Step40;
    y = (int64_t)floor(xl + log(v) / laml);
    if (y < 0 || v == 0.0) goto Step10;
    v = v * (u - p2) * laml;
    goto Step50;
Step40:
    y = (int64_t)floor(xr - log(v) / lamr);
    if (y > n || v == 0.0) goto Step10;
    v = v * (u - p3) * lamr;
Step50:
    k = llabs(y - m);
    if (k > 20 && k < (nrq / 2.0 - 1)) goto Step52;
    s = rr / q;
    a = s * (n + 1);
    F = 1.0;
    if (m < y) {
        for (i = m + 1; i <= y; i++) F *= (a / i - s);
    } else if (m > y) {
        for (i = y + 1; i <= m; i++) F /= (a / i - s);
    }
    if (v > F) goto Step10;
    goto Step60;
Step52:
    rho = (k / nrq) * ((k * (k / 3.0 + 0.625) + 0.16666666666666666) / nrq + 0.5);
    t = -k * k / (2 * nrq);
    A = log(v);
    if (A < (t - rho)) goto Step60;
    if (A > (t + rho)) goto Step10;
    x1 = (double)y + 1;
    f1 = (double)m + 1;
    z = (double)n + 1 - (double)m;
    w = (double)n - (double)y + 1;
    x2 = x1 * x1;
    f2 = f1 * f1;
    z2 = z * z;
    w2 = w * w;
    if (A > (xm * log(f1 / x1) + (n - m + 0.5) * log(z / w) +
             (y - m) * log(w * rr / (x1 * q)) +
             (13860. - (462. - (132. - (99. - 140. / f2) / f2) / f2) / f2) / f1 / 166320. +
             (13860. - (462. - (132. - (99. - 140. / z2) / z2) / z2) / z2) / z / 166320. -
             (13860. - (462. - (132. - (99. - 140. / x2) / x2) / x2) / x2) / x1 / 166320. -
             (13860. - (462. - (132. - (99. - 140. / w2) / w2) / w2) / w2) / w / 166320.))
        goto Step10;
Step60:
    if (p > 0.5) y = n - y;
    return y;
}
static int64_t zap_gen_binomial(ZapRng *r, int64_t n, double p) {
    double q;
    if (n == 0 || p == 0.0) return 0;
    if (p <= 0.5) {
        if (p * n <= 30.0) return zap_gen_binom_inv(r, n, p);
        return zap_gen_binom_btpe(r, n, p);
    }
    q = 1.0 - p;
    if (q * n <= 30.0) return n - zap_gen_binom_inv(r, n, q);
    return n - zap_gen_binom_btpe(r, n, q);
}

/* ---- public scalar entry points ---- */
double zap_random_gen_random(void *g) { return zap_rng_nextd((ZapRng *)g); }
double zap_random_gen_uniform(void *g, double lo, double hi) {
    return lo + (hi - lo) * zap_rng_nextd((ZapRng *)g);
}
int64_t zap_random_gen_integers(void *g, int64_t lo, int64_t hi, int64_t endpoint) {
    return zap_gen_integer((ZapRng *)g, lo, hi, endpoint);
}
double zap_random_gen_standard_normal(void *g) { return zap_gen_std_normal((ZapRng *)g); }
double zap_random_gen_normal(void *g, double loc, double scale) {
    return loc + scale * zap_gen_std_normal((ZapRng *)g);
}
double zap_random_gen_standard_exponential(void *g) { return zap_gen_std_exp((ZapRng *)g); }
double zap_random_gen_exponential(void *g, double scale) {
    return scale * zap_gen_std_exp((ZapRng *)g);
}
double zap_random_gen_standard_gamma(void *g, double shape) {
    return zap_gen_std_gamma((ZapRng *)g, shape);
}
double zap_random_gen_gamma(void *g, double shape, double scale) {
    return scale * zap_gen_std_gamma((ZapRng *)g, shape);
}
double zap_random_gen_beta(void *g, double a, double b) {
    return zap_gen_beta((ZapRng *)g, a, b);
}
int64_t zap_random_gen_poisson(void *g, double lam) { return zap_gen_poisson((ZapRng *)g, lam); }
int64_t zap_random_gen_binomial(void *g, int64_t n, double p) {
    return zap_gen_binomial((ZapRng *)g, n, p);
}

/* ---- public fill entry points (arr already shaped; count = total) ---- */
static double *zap_rng_arr_f64(void *arr, int64_t *count) {
    ZapArray *a = (ZapArray *)arr;
    *count = zap_array_count_of(a);
    return (double *)a->data + a->off;
}
static int64_t *zap_rng_arr_i64(void *arr, int64_t *count) {
    ZapArray *a = (ZapArray *)arr;
    *count = zap_array_count_of(a);
    return (int64_t *)a->data + a->off;
}
void zap_random_gen_random_fill(void *g, void *arr) {
    int64_t n; double *d = zap_rng_arr_f64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = zap_rng_nextd((ZapRng *)g);
}
void zap_random_gen_uniform_fill(void *g, void *arr, double lo, double hi) {
    int64_t n; double *d = zap_rng_arr_f64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = lo + (hi - lo) * zap_rng_nextd((ZapRng *)g);
}
void zap_random_gen_integers_fill(void *g, void *arr, int64_t lo, int64_t hi, int64_t endpoint) {
    int64_t n; int64_t *d = zap_rng_arr_i64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = zap_gen_integer((ZapRng *)g, lo, hi, endpoint);
}
void zap_random_gen_standard_normal_fill(void *g, void *arr) {
    int64_t n; double *d = zap_rng_arr_f64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = zap_gen_std_normal((ZapRng *)g);
}
void zap_random_gen_normal_fill(void *g, void *arr, double loc, double scale) {
    int64_t n; double *d = zap_rng_arr_f64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = loc + scale * zap_gen_std_normal((ZapRng *)g);
}
void zap_random_gen_standard_exponential_fill(void *g, void *arr) {
    int64_t n; double *d = zap_rng_arr_f64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = zap_gen_std_exp((ZapRng *)g);
}
void zap_random_gen_exponential_fill(void *g, void *arr, double scale) {
    int64_t n; double *d = zap_rng_arr_f64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = scale * zap_gen_std_exp((ZapRng *)g);
}
void zap_random_gen_standard_gamma_fill(void *g, void *arr, double shape) {
    int64_t n; double *d = zap_rng_arr_f64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = zap_gen_std_gamma((ZapRng *)g, shape);
}
void zap_random_gen_gamma_fill(void *g, void *arr, double shape, double scale) {
    int64_t n; double *d = zap_rng_arr_f64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = scale * zap_gen_std_gamma((ZapRng *)g, shape);
}
void zap_random_gen_beta_fill(void *g, void *arr, double a, double b) {
    int64_t n; double *d = zap_rng_arr_f64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = zap_gen_beta((ZapRng *)g, a, b);
}
void zap_random_gen_poisson_fill(void *g, void *arr, double lam) {
    int64_t n; int64_t *d = zap_rng_arr_i64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = zap_gen_poisson((ZapRng *)g, lam);
}
void zap_random_gen_binomial_fill(void *g, void *arr, int64_t nn, double p) {
    int64_t n; int64_t *d = zap_rng_arr_i64(arr, &n);
    for (int64_t i = 0; i < n; i++) d[i] = zap_gen_binomial((ZapRng *)g, nn, p);
}

/* ---- choice (NumPy Generator.choice; a is an int population) ---- */
/* _shuffle_int: Fisher-Yates over an i64 index buffer via the Lemire bounded
 * scalar (NumPy _generator.pyx). */
static void zap_rng_shuffle_int(ZapRng *r, int64_t *d, int64_t n, int64_t first) {
    for (int64_t i = n - 1; i >= first; i--) {
        int64_t j = (int64_t)zap_rng_bounded(r, 0, (uint64_t)i);
        int64_t t = d[j]; d[j] = d[i]; d[i] = t;
    }
}
/* searchsorted(cdf, u, 'right'): first i with cdf[i] > u. */
static int64_t zap_rng_searchsorted_right(const double *cdf, int64_t pop, double u) {
    int64_t lo = 0, hi = pop;
    while (lo < hi) {
        int64_t mid = lo + (hi - lo) / 2;
        if (cdf[mid] > u) hi = mid; else lo = mid + 1;
    }
    return lo;
}
/* Fill out (i64) with `count` indices in [0, pop). replace / p / shuffle match
 * NumPy Generator.choice exactly. `p` is a ZapArray[f64] of length pop, or NULL
 * for uniform. Aborts on misuse (NumPy raises ValueError; catchable is a
 * follow-up). */
void zap_random_gen_choice_fill(void *g, void *arr_i64, int64_t pop, int64_t replace,
                                void *p_or_null, int64_t shuffle) {
    ZapRng *r = (ZapRng *)g;
    int64_t count;
    int64_t *out = zap_rng_arr_i64(arr_i64, &count);
    const double *p = NULL;
    if (p_or_null) {
        ZapArray *pa = (ZapArray *)p_or_null;
        p = (const double *)pa->data + pa->off;
    }
    if (replace) {
        if (p == NULL) {
            for (int64_t i = 0; i < count; i++)
                out[i] = zap_gen_integer(r, 0, pop, 0);
        } else {
            double *cdf = (double *)malloc((size_t)pop * sizeof(double));
            double s = 0;
            for (int64_t i = 0; i < pop; i++) { s += p[i]; cdf[i] = s; }
            for (int64_t i = 0; i < pop; i++) cdf[i] /= cdf[pop - 1];
            for (int64_t k = 0; k < count; k++)
                out[k] = zap_rng_searchsorted_right(cdf, pop, zap_rng_nextd(r));
            free(cdf);
        }
        return;
    }
    /* replace == 0 */
    if (count > pop) {
        fprintf(stderr, "zap: random.choice: sample larger than population without replacement\n");
        abort();
    }
    if (p != NULL) {
        double *pp = (double *)malloc((size_t)pop * sizeof(double));
        memcpy(pp, p, (size_t)pop * sizeof(double));
        double *cdf = (double *)malloc((size_t)pop * sizeof(double));
        int64_t n_uniq = 0;
        while (n_uniq < count) {
            int64_t m = count - n_uniq;
            double *x = (double *)malloc((size_t)m * sizeof(double));
            for (int64_t i = 0; i < m; i++) x[i] = zap_rng_nextd(r);
            if (n_uniq > 0)
                for (int64_t i = 0; i < n_uniq; i++) pp[out[i]] = 0;
            double s = 0;
            for (int64_t i = 0; i < pop; i++) { s += pp[i]; cdf[i] = s; }
            for (int64_t i = 0; i < pop; i++) cdf[i] /= cdf[pop - 1];
            int64_t *nw = (int64_t *)malloc((size_t)m * sizeof(int64_t));
            for (int64_t j = 0; j < m; j++)
                nw[j] = zap_rng_searchsorted_right(cdf, pop, x[j]);
            /* np.unique(new) + first-occurrence order: keep nw[j] iff it did not
             * already appear earlier in this round's draws. */
            for (int64_t j = 0; j < m; j++) {
                int dup = 0;
                for (int64_t t = 0; t < j; t++)
                    if (nw[t] == nw[j]) { dup = 1; break; }
                if (!dup) out[n_uniq++] = nw[j];
            }
            free(nw);
            free(x);
        }
        free(cdf);
        free(pp);
        return;
    }
    /* replace == 0, p == NULL */
    int64_t cutoff = shuffle ? 50 : 20;
    if (pop > 10000 && count > (pop / cutoff)) {
        /* tail-shuffle: shuffle the last `count` of arange(pop), take them */
        int64_t *idx = (int64_t *)malloc((size_t)pop * sizeof(int64_t));
        for (int64_t i = 0; i < pop; i++) idx[i] = i;
        int64_t first = pop - count; if (first < 1) first = 1;
        zap_rng_shuffle_int(r, idx, pop, first);
        for (int64_t i = 0; i < count; i++) out[i] = idx[pop - count + i];
        free(idx);
        return;
    }
    /* Floyd's algorithm with an open-addressed hash set (NumPy). */
    uint64_t set_size = (uint64_t)(1.2 * (double)count);
    uint64_t mask = zap_gen_mask(set_size);
    set_size = 1 + mask;
    uint64_t *hs = (uint64_t *)malloc((size_t)set_size * sizeof(uint64_t));
    for (uint64_t i = 0; i < set_size; i++) hs[i] = (uint64_t)-1;
    for (int64_t j = pop - count; j < pop; j++) {
        uint64_t val = zap_rng_bounded(r, 0, (uint64_t)j);
        uint64_t loc = val & mask;
        while (hs[loc] != (uint64_t)-1 && hs[loc] != val) loc = (loc + 1) & mask;
        if (hs[loc] == (uint64_t)-1) {
            hs[loc] = val;
            out[j - pop + count] = (int64_t)val;
        } else {
            loc = (uint64_t)j & mask;
            while (hs[loc] != (uint64_t)-1) loc = (loc + 1) & mask;
            hs[loc] = (uint64_t)j;
            out[j - pop + count] = j;
        }
    }
    if (shuffle) zap_rng_shuffle_int(r, out, count, 1);
    free(hs);
}

/* ---- shuffle / permutation (axis 0; NumPy _shuffle_raw with random_interval) ---- */
/* In-place Fisher-Yates over the first axis, swapping `block`-element rows. */
static void zap_rng_shuffle_blocks(ZapRng *r, void *arr) {
    ZapArray *a = (ZapArray *)arr;
    int64_t len = a->dims[0];
    if (len <= 1) return;
    int64_t total = zap_array_count_of(a);
    int64_t block = len ? total / len : 0; /* elements per row */
    size_t esz = zap_array_elem_size(a->dtype);
    char *base = (char *)a->data + (size_t)a->off * esz;
    char tmp[16]; /* max element width (complex128) */
    for (int64_t i = len - 1; i > 0; i--) {
        int64_t j = (int64_t)zap_rng_interval(r, (uint64_t)i);
        if (j == i) continue;
        char *ri = base + (size_t)(i * block) * esz;
        char *rj = base + (size_t)(j * block) * esz;
        for (int64_t e = 0; e < block; e++) {
            memcpy(tmp, ri + (size_t)e * esz, esz);
            memcpy(ri + (size_t)e * esz, rj + (size_t)e * esz, esz);
            memcpy(rj + (size_t)e * esz, tmp, esz);
        }
    }
}
void zap_random_gen_shuffle(void *g, void *arr) { zap_rng_shuffle_blocks((ZapRng *)g, arr); }

/* permutation(n): a freshly shuffled arange(n) as a 1-D i64 array. */
void *zap_random_gen_permutation_n(void *g, int64_t n) {
    void *arr = zap_array_new(DTYPE_I64, n);
    int64_t *d = (int64_t *)((ZapArray *)arr)->data;
    for (int64_t i = 0; i < n; i++) d[i] = i;
    zap_rng_shuffle_blocks((ZapRng *)g, arr);
    return arr;
}
/* permutation(array): a shuffled copy of `src` (same dtype/shape, axis 0). */
void *zap_random_gen_permutation(void *g, void *src) {
    ZapArray *s = (ZapArray *)src;
    void *arr;
    int64_t total = zap_array_count_of(s);
    if (s->ndim == 1) {
        arr = zap_array_new(s->dtype, s->dims[0]);
    } else {
        arr = zap_array_new_nd(s->dtype, s->ndim, total);
        for (int64_t k = 0; k < s->ndim; k++) zap_array_set_dim(arr, k, s->dims[k]);
    }
    ZapArray *d = (ZapArray *)arr;
    size_t esz = zap_array_elem_size(s->dtype);
    /* copy honoring a possible source view (off/strides) element-by-element */
    for (int64_t i = 0; i < total; i++) {
        int64_t ph = zap_array_phys(s, i);
        memcpy((char *)d->data + (size_t)i * esz,
               (char *)s->data + (size_t)ph * esz, esz);
    }
    zap_rng_shuffle_blocks((ZapRng *)g, arr);
    return arr;
}
