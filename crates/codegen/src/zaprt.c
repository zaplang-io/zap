/* Minimal Zap runtime (Phase 1).
 *
 * String entry points per design/RUNTIME-ABI.md, and a two-word string object
 * { len, data }. Heap allocation routes through the conservative Boehm
 * collector (ticket #20, D-0009): `GC_MALLOC` for objects that may hold
 * pointers (scanned) and `GC_MALLOC_ATOMIC` for pointer-free byte buffers. The
 * collector is initialized once at startup via a constructor; it scans the
 * stack, registers and globals, so no stack maps or safepoints are needed. The
 * type-descriptor header from the ABI is added when reflection / precise GC
 * land. */
/* Python.h must precede the standard headers (it sets feature-test macros).
 * Embedding CPython backs the Python interop (ticket #27, D-0012); the compile
 * flags come from `pkg-config --cflags python3-embed` (see build_executable),
 * and the dev container / CI provide `python3-dev`. */
#define PY_SSIZE_T_CLEAN
#include <Python.h>

/* `GC_THREADS` must be defined before <gc.h> so the collector is built/used in
 * its thread-aware mode (ticket #25, D-0009/D-0013): it then knows about every
 * goroutine OS thread and scans each one's stack and registers, so live objects
 * reachable only from a running or suspended coroutine are never collected. The
 * goroutine runtime starts threads with `GC_pthread_create` (declared by <gc.h>
 * in this mode) so the collector registers them automatically; channels block
 * on pthread mutex/condvars. The dev container's `libgc-dev` has thread
 * support, and the program links `-lpthread` (see build_executable). */
#define GC_THREADS
#include <gc.h>
#include <math.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* PCRE2, 8-bit code units (ticket #23, D-0010). Must be defined before the
 * header so the `pcre2_*` names resolve to the 8-bit API. Linked with
 * `-lpcre2-8` (see build_executable). */
#define PCRE2_CODE_UNIT_WIDTH 8
#include <pcre2.h>

typedef struct {
    int64_t len;
    const char *data;
} ZapStr;

/* Initialize the Boehm collector exactly once, before any Zap code runs. */
__attribute__((constructor)) static void zap_gc_init(void) {
    GC_INIT();
}

/* Allocate `size` zeroed, pointer-scanned bytes via the GC. */
void *zap_alloc(int64_t size) {
    return GC_MALLOC((size_t)size);
}

void *zap_str_new(const char *data, int64_t len) {
    ZapStr *s = (ZapStr *)GC_MALLOC(sizeof(ZapStr));
    char *buf = (char *)GC_MALLOC_ATOMIC((size_t)len);
    if (len > 0) {
        memcpy(buf, data, (size_t)len);
    }
    s->len = len;
    s->data = buf;
    return s;
}

void *zap_str_concat(void *a, void *b) {
    ZapStr *x = (ZapStr *)a;
    ZapStr *y = (ZapStr *)b;
    int64_t n = x->len + y->len;
    char *buf = (char *)GC_MALLOC_ATOMIC((size_t)n);
    memcpy(buf, x->data, (size_t)x->len);
    memcpy(buf + x->len, y->data, (size_t)y->len);
    ZapStr *s = (ZapStr *)GC_MALLOC(sizeof(ZapStr));
    s->len = n;
    s->data = buf;
    return s;
}

void zap_print_str(void *s) {
    ZapStr *z = (ZapStr *)s;
    fwrite(z->data, 1, (size_t)z->len, stdout);
}

/* --- scalar numeric helpers (ticket #19 / T9) ---
 *
 * `**` (power) and `//` (floor division) lower to these calls so codegen stays
 * mechanical and the semantics live in one place. */

/* Integer power by binary exponentiation. The result stays integer (the static
 * type of `int ** int`, D-0019), so a negative exponent yields the floor of the
 * true fractional power: 0 for |base| > 1, and the sign-tracking +/-1 for
 * base == -1. Overflow wraps (two's complement, D-0019). */
int64_t zap_ipow(int64_t base, int64_t exp) {
    if (exp < 0) {
        if (base == 1) {
            return 1;
        }
        if (base == -1) {
            return (exp & 1) ? -1 : 1;
        }
        return 0;
    }
    int64_t result = 1;
    int64_t b = base;
    uint64_t e = (uint64_t)exp;
    while (e != 0) {
        if (e & 1u) {
            result *= b;
        }
        e >>= 1;
        if (e != 0) {
            b *= b;
        }
    }
    return result;
}

/* Floating-point power via libm. */
double zap_powf(double base, double exp) { return pow(base, exp); }

/* Floor division for signed integers (Python semantics, D-0024): rounds toward
 * negative infinity. C `/` truncates toward zero, so correct the quotient when
 * the remainder is non-zero and the operand signs differ. Division by zero is a
 * fault (aborts, matching the index-out-of-range behaviour until the typed
 * exception runtime lands). */
int64_t zap_ifloordiv(int64_t a, int64_t b) {
    if (b == 0) {
        fprintf(stderr, "zap: integer division or modulo by zero\n");
        abort();
    }
    int64_t q = a / b;
    int64_t r = a % b;
    if (r != 0 && ((r < 0) != (b < 0))) {
        q -= 1;
    }
    return q;
}

/* Floor division for floats (Python semantics, D-0024): floor(a / b). Matches
 * CPython's result for finite operands; division by zero follows IEEE-754
 * (+/-inf or NaN) rather than faulting. */
double zap_ffloordiv(double a, double b) { return floor(a / b); }

int64_t zap_str_len(void *s) { return ((ZapStr *)s)->len; }

/* ---------------------------------------------------------------------------
 * Collections (ticket #21, design/RUNTIME-ABI.md §"Collection layout").
 *
 * Elements, keys and values are stored as uniform 8-byte words so one core
 * implementation serves every element type: integers/bools fit directly, and
 * reference types (str, structs, nested collections) fit as pointer bits. The
 * typed `_i64` / `_ptr` / `_str` wrappers below are the ABI surface the
 * compiler calls; they only convert at the boundary and delegate to the core.
 *
 * Buffers are allocated with GC_MALLOC (pointer-scanned, not atomic) so stored
 * reference words keep their targets alive under the conservative collector.
 * ------------------------------------------------------------------------- */

/* list[T]: [ len | cap | data ], amortized-doubling growth. */
typedef struct {
    int64_t len;
    int64_t cap;
    int64_t *data; /* `cap` element words */
} ZapList;

void *zap_list_new(void) {
    ZapList *l = (ZapList *)GC_MALLOC(sizeof(ZapList));
    l->len = 0;
    l->cap = 0;
    l->data = NULL;
    return l;
}

static void zap_list_grow(ZapList *l) {
    int64_t cap = l->cap < 4 ? 4 : l->cap * 2;
    int64_t *data = (int64_t *)GC_MALLOC((size_t)cap * sizeof(int64_t));
    if (l->len > 0) {
        memcpy(data, l->data, (size_t)l->len * sizeof(int64_t));
    }
    l->data = data;
    l->cap = cap;
}

static void zap_list_push(ZapList *l, int64_t word) {
    if (l->len == l->cap) {
        zap_list_grow(l);
    }
    l->data[l->len++] = word;
}

/* Out-of-range access aborts (no exception runtime yet, ticket #22). */
static int64_t zap_list_at(ZapList *l, int64_t i) {
    if (i < 0 || i >= l->len) {
        fprintf(stderr, "zap: list index %lld out of range (len %lld)\n",
                (long long)i, (long long)l->len);
        abort();
    }
    return l->data[i];
}

void zap_list_push_i64(void *l, int64_t v) { zap_list_push((ZapList *)l, v); }
void zap_list_push_ptr(void *l, void *v) {
    zap_list_push((ZapList *)l, (int64_t)(intptr_t)v);
}
int64_t zap_list_get_i64(void *l, int64_t i) { return zap_list_at((ZapList *)l, i); }
void *zap_list_get_ptr(void *l, int64_t i) {
    return (void *)(intptr_t)zap_list_at((ZapList *)l, i);
}
void zap_list_set_i64(void *l, int64_t i, int64_t v) {
    (void)zap_list_at((ZapList *)l, i); /* bounds-check */
    ((ZapList *)l)->data[i] = v;
}
void zap_list_set_ptr(void *l, int64_t i, void *v) {
    (void)zap_list_at((ZapList *)l, i); /* bounds-check */
    ((ZapList *)l)->data[i] = (int64_t)(intptr_t)v;
}
int64_t zap_list_len(void *l) { return ((ZapList *)l)->len; }

/* dict[K, V]: open-addressing hash table with linear probing.
 * Key kind (int64 vs str) is passed per call; a dict is homogeneous, so this is
 * stable across a given dict's lifetime. */
enum { KEY_I64 = 0, KEY_STR = 1 };
enum { SLOT_EMPTY = 0, SLOT_FULL = 1, SLOT_TOMB = 2 };

typedef struct {
    uint8_t state;
    int64_t key; /* key word (int64, or ZapStr* bits) */
    int64_t val; /* value word */
} ZapEntry;

typedef struct {
    int64_t len;
    int64_t cap; /* number of slots; always a power of two or 0 */
    ZapEntry *entries;
} ZapDict;

void *zap_dict_new(void) {
    ZapDict *d = (ZapDict *)GC_MALLOC(sizeof(ZapDict));
    d->len = 0;
    d->cap = 0;
    d->entries = NULL;
    return d;
}

static uint64_t zap_hash_i64(int64_t k) {
    /* splitmix64 finalizer — cheap, good dispersion. */
    uint64_t x = (uint64_t)k + 0x9e3779b97f4a7c15ULL;
    x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ULL;
    x = (x ^ (x >> 27)) * 0x94d049bb133111ebULL;
    return x ^ (x >> 31);
}

static uint64_t zap_hash_str(const ZapStr *s) {
    uint64_t h = 1469598103934665603ULL; /* FNV-1a */
    for (int64_t i = 0; i < s->len; i++) {
        h ^= (uint8_t)s->data[i];
        h *= 1099511628211ULL;
    }
    return h;
}

static uint64_t zap_key_hash(int64_t key, int keykind) {
    return keykind == KEY_STR ? zap_hash_str((const ZapStr *)(intptr_t)key)
                              : zap_hash_i64(key);
}

static int zap_key_eq(int64_t a, int64_t b, int keykind) {
    if (keykind != KEY_STR) {
        return a == b;
    }
    const ZapStr *x = (const ZapStr *)(intptr_t)a;
    const ZapStr *y = (const ZapStr *)(intptr_t)b;
    return x->len == y->len && memcmp(x->data, y->data, (size_t)x->len) == 0;
}

static void zap_dict_resize(ZapDict *d, int keykind) {
    int64_t ncap = d->cap < 8 ? 8 : d->cap * 2;
    ZapEntry *ne = (ZapEntry *)GC_MALLOC((size_t)ncap * sizeof(ZapEntry));
    for (int64_t i = 0; i < ncap; i++) {
        ne[i].state = SLOT_EMPTY;
    }
    for (int64_t i = 0; i < d->cap; i++) {
        if (d->entries[i].state != SLOT_FULL) {
            continue;
        }
        uint64_t h = zap_key_hash(d->entries[i].key, keykind);
        int64_t j = (int64_t)(h & (uint64_t)(ncap - 1));
        while (ne[j].state == SLOT_FULL) {
            j = (j + 1) & (ncap - 1);
        }
        ne[j] = d->entries[i];
    }
    d->entries = ne;
    d->cap = ncap;
}

static void zap_dict_set(void *dp, int64_t key, int keykind, int64_t val) {
    ZapDict *d = (ZapDict *)dp;
    if (d->cap == 0 || (d->len + 1) * 4 >= d->cap * 3) { /* load factor 0.75 */
        zap_dict_resize(d, keykind);
    }
    uint64_t h = zap_key_hash(key, keykind);
    int64_t j = (int64_t)(h & (uint64_t)(d->cap - 1));
    int64_t tomb = -1;
    while (d->entries[j].state != SLOT_EMPTY) {
        if (d->entries[j].state == SLOT_FULL &&
            zap_key_eq(d->entries[j].key, key, keykind)) {
            d->entries[j].val = val; /* overwrite existing key */
            return;
        }
        if (d->entries[j].state == SLOT_TOMB && tomb < 0) {
            tomb = j;
        }
        j = (j + 1) & (d->cap - 1);
    }
    if (tomb >= 0) {
        j = tomb;
    }
    d->entries[j].state = SLOT_FULL;
    d->entries[j].key = key;
    d->entries[j].val = val;
    d->len++;
}

/* Missing-key get returns 0 (null word) for this increment; membership and a
 * real KeyError wait for the exception runtime (ticket #22). */
static int64_t zap_dict_get(void *dp, int64_t key, int keykind) {
    ZapDict *d = (ZapDict *)dp;
    if (d->cap == 0) {
        return 0;
    }
    uint64_t h = zap_key_hash(key, keykind);
    int64_t j = (int64_t)(h & (uint64_t)(d->cap - 1));
    while (d->entries[j].state != SLOT_EMPTY) {
        if (d->entries[j].state == SLOT_FULL &&
            zap_key_eq(d->entries[j].key, key, keykind)) {
            return d->entries[j].val;
        }
        j = (j + 1) & (d->cap - 1);
    }
    return 0;
}

int64_t zap_dict_len(void *d) { return ((ZapDict *)d)->len; }

void zap_dict_set_i64_i64(void *d, int64_t k, int64_t v) {
    zap_dict_set(d, k, KEY_I64, v);
}
void zap_dict_set_i64_ptr(void *d, int64_t k, void *v) {
    zap_dict_set(d, k, KEY_I64, (int64_t)(intptr_t)v);
}
void zap_dict_set_str_i64(void *d, void *k, int64_t v) {
    zap_dict_set(d, (int64_t)(intptr_t)k, KEY_STR, v);
}
void zap_dict_set_str_ptr(void *d, void *k, void *v) {
    zap_dict_set(d, (int64_t)(intptr_t)k, KEY_STR, (int64_t)(intptr_t)v);
}
int64_t zap_dict_get_i64_i64(void *d, int64_t k) {
    return zap_dict_get(d, k, KEY_I64);
}
void *zap_dict_get_i64_ptr(void *d, int64_t k) {
    return (void *)(intptr_t)zap_dict_get(d, k, KEY_I64);
}
int64_t zap_dict_get_str_i64(void *d, void *k) {
    return zap_dict_get(d, (int64_t)(intptr_t)k, KEY_STR);
}
void *zap_dict_get_str_ptr(void *d, void *k) {
    return (void *)(intptr_t)zap_dict_get(d, (int64_t)(intptr_t)k, KEY_STR);
}

/* set[T]: a dict without values — open-addressing hash table of element words.
 * Shares the key hashing/equality above (RUNTIME-ABI: "dict and set can share
 * hashing"). Elements are uniform 8-byte words, like dict keys; the element
 * kind (int64 vs str) is passed per call and is stable for a given set. */
typedef struct {
    uint8_t state;
    int64_t key; /* element word (int64, or ZapStr* bits) */
} ZapSetEntry;

typedef struct {
    int64_t len;
    int64_t cap; /* number of slots; always a power of two or 0 */
    ZapSetEntry *entries;
} ZapSet;

void *zap_set_new(void) {
    ZapSet *s = (ZapSet *)GC_MALLOC(sizeof(ZapSet));
    s->len = 0;
    s->cap = 0;
    s->entries = NULL;
    return s;
}

static void zap_set_resize(ZapSet *s, int keykind) {
    int64_t ncap = s->cap < 8 ? 8 : s->cap * 2;
    ZapSetEntry *ne = (ZapSetEntry *)GC_MALLOC((size_t)ncap * sizeof(ZapSetEntry));
    for (int64_t i = 0; i < ncap; i++) {
        ne[i].state = SLOT_EMPTY;
    }
    for (int64_t i = 0; i < s->cap; i++) {
        if (s->entries[i].state != SLOT_FULL) {
            continue;
        }
        uint64_t h = zap_key_hash(s->entries[i].key, keykind);
        int64_t j = (int64_t)(h & (uint64_t)(ncap - 1));
        while (ne[j].state == SLOT_FULL) {
            j = (j + 1) & (ncap - 1);
        }
        ne[j] = s->entries[i];
    }
    s->entries = ne;
    s->cap = ncap;
}

static void zap_set_add(void *sp, int64_t key, int keykind) {
    ZapSet *s = (ZapSet *)sp;
    if (s->cap == 0 || (s->len + 1) * 4 >= s->cap * 3) { /* load factor 0.75 */
        zap_set_resize(s, keykind);
    }
    uint64_t h = zap_key_hash(key, keykind);
    int64_t j = (int64_t)(h & (uint64_t)(s->cap - 1));
    while (s->entries[j].state != SLOT_EMPTY) {
        if (s->entries[j].state == SLOT_FULL &&
            zap_key_eq(s->entries[j].key, key, keykind)) {
            return; /* already present */
        }
        j = (j + 1) & (s->cap - 1);
    }
    s->entries[j].state = SLOT_FULL;
    s->entries[j].key = key;
    s->len++;
}

static int64_t zap_set_contains(void *sp, int64_t key, int keykind) {
    ZapSet *s = (ZapSet *)sp;
    if (s->cap == 0) {
        return 0;
    }
    uint64_t h = zap_key_hash(key, keykind);
    int64_t j = (int64_t)(h & (uint64_t)(s->cap - 1));
    while (s->entries[j].state != SLOT_EMPTY) {
        if (s->entries[j].state == SLOT_FULL &&
            zap_key_eq(s->entries[j].key, key, keykind)) {
            return 1;
        }
        j = (j + 1) & (s->cap - 1);
    }
    return 0;
}

int64_t zap_set_len(void *s) { return ((ZapSet *)s)->len; }

void zap_set_add_i64(void *s, int64_t v) { zap_set_add(s, v, KEY_I64); }
void zap_set_add_str(void *s, void *v) {
    zap_set_add(s, (int64_t)(intptr_t)v, KEY_STR);
}
int64_t zap_set_contains_i64(void *s, int64_t v) {
    return zap_set_contains(s, v, KEY_I64);
}
int64_t zap_set_contains_str(void *s, void *v) {
    return zap_set_contains(s, (int64_t)(intptr_t)v, KEY_STR);
}

/* Snapshot the live elements into a fresh list (element words copied verbatim),
 * so `for x in s` can reuse the list iterator. Iteration order is unspecified. */
void *zap_set_items(void *sp) {
    ZapSet *s = (ZapSet *)sp;
    ZapList *out = (ZapList *)zap_list_new();
    for (int64_t i = 0; i < s->cap; i++) {
        if (s->entries[i].state == SLOT_FULL) {
            zap_list_push(out, s->entries[i].key);
        }
    }
    return out;
}

/* ---------------------------------------------------------------------------
 * Numeric arrays (ticket #24, design/0002-arrays.md, design/RUNTIME-ABI.md
 * §"Collection layout": `array[T]`).
 *
 * A dense, contiguous, fixed-dtype buffer. The first increment is 1-D over the
 * dtypes `f64` and `i64` (both 8-byte element words). The object carries its
 * dtype and shape so N-D (`ndim > 1`) and strided views can layer on later
 * without an ABI break; this increment always sets `ndim = 1`.
 *
 * The element buffer is pointer-free, so it is allocated with GC_MALLOC_ATOMIC
 * (the collector need not scan it); the shape (`dims`) is likewise atomic. The
 * element-wise kernels (`a + b`, `a * 2.0`, ...) are *not* here: codegen lowers
 * them to explicit LLVM vector operations (SIMD) over `zap_array_data` buffers,
 * which is the point of ticket #24. The runtime only provides construction,
 * element access, length, and scalar reductions.
 *
 * Deferred (clean follow-ups): N-D shape + row-major indexing, strides/views
 * (D-0030), broadcasting beyond a scalar, dtype promotion, and shape-mismatch
 * runtime checks on element-wise ops. */
enum { DTYPE_F64 = 0, DTYPE_I64 = 1 };

typedef struct {
    int64_t dtype;  /* DTYPE_F64 | DTYPE_I64 */
    int64_t ndim;   /* always 1 this increment */
    int64_t *dims;  /* `ndim` dimensions; dims[0] is the length */
    void *data;     /* contiguous row-major buffer, len * 8 bytes */
} ZapArray;

void *zap_array_new(int64_t dtype, int64_t len) {
    ZapArray *a = (ZapArray *)GC_MALLOC(sizeof(ZapArray));
    int64_t *dims = (int64_t *)GC_MALLOC_ATOMIC(sizeof(int64_t));
    dims[0] = len;
    /* f64 and i64 are both 8-byte words; the buffer holds no pointers. */
    void *data = GC_MALLOC_ATOMIC((size_t)(len > 0 ? len : 0) * 8);
    a->dtype = dtype;
    a->ndim = 1;
    a->dims = dims;
    a->data = data;
    return a;
}

int64_t zap_array_len(void *a) { return ((ZapArray *)a)->dims[0]; }

/* The raw element buffer; codegen's SIMD kernels index this directly. */
void *zap_array_data(void *a) { return ((ZapArray *)a)->data; }

/* Out-of-range access aborts (shape-mismatch exceptions wait for a follow-up,
 * matching the list bounds behaviour above). */
static void zap_array_bounds(ZapArray *a, int64_t i) {
    if (i < 0 || i >= a->dims[0]) {
        fprintf(stderr, "zap: array index %lld out of range (len %lld)\n",
                (long long)i, (long long)a->dims[0]);
        abort();
    }
}

double zap_array_get_f64(void *ap, int64_t i) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    return ((double *)a->data)[i];
}
void zap_array_set_f64(void *ap, int64_t i, double v) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    ((double *)a->data)[i] = v;
}
int64_t zap_array_get_i64(void *ap, int64_t i) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    return ((int64_t *)a->data)[i];
}
void zap_array_set_i64(void *ap, int64_t i, int64_t v) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    ((int64_t *)a->data)[i] = v;
}

double zap_array_sum_f64(void *ap) {
    ZapArray *a = (ZapArray *)ap;
    const double *d = (const double *)a->data;
    double s = 0.0;
    for (int64_t i = 0; i < a->dims[0]; i++) {
        s += d[i];
    }
    return s;
}
int64_t zap_array_sum_i64(void *ap) {
    ZapArray *a = (ZapArray *)ap;
    const int64_t *d = (const int64_t *)a->data;
    int64_t s = 0;
    for (int64_t i = 0; i < a->dims[0]; i++) {
        s += d[i];
    }
    return s;
}

/* ---------------------------------------------------------------------------
 * Exceptions / unwinding (ticket #22, design/RUNTIME-ABI.md §"Exception /
 * unwinding ABI"). Zap uses the platform Itanium unwinder: `zap_panic` raises,
 * landing pads catch via the libstdc++ personality (`__gxx_personality_v0`,
 * emitted by codegen), and `except` matching is catch-all for this increment.
 *
 * The thrown C++ exception just boxes a pointer to the Zap exception object,
 * which is currently the message string itself (a ZapStr*); the typed Zap
 * exception hierarchy (D-0029) and type-directed `except` dispatch are deferred.
 * Keeping all the C++-ABI glue here lets codegen stay a mechanical ZIR->LLVM
 * translator (it only emits `invoke`/`landingpad` and names these entry points).
 * ------------------------------------------------------------------------- */

/* libstdc++/libc++abi entry points (declared here, linked via -lstdc++). */
extern void *__cxa_allocate_exception(size_t size);
extern void __cxa_throw(void *thrown, void *type_info, void (*dest)(void *));
extern void *__cxa_begin_catch(void *exc);
extern void __cxa_end_catch(void);

/* A minimal `std::type_info` for Zap exceptions: its vtable pointer is that of
 * libstdc++'s `__cxxabiv1::__class_type_info` (past the two reserved slots), so
 * the personality treats it as an ordinary class type. A catch-all landing-pad
 * clause (`catch ptr null`) matches it regardless of its name. */
extern const void *const _ZTVN10__cxxabiv117__class_type_infoE[];
typedef struct {
    const void *vtable;
    const char *name;
} ZapTypeInfo;
const ZapTypeInfo zap_exception_type_info = {
    (const void *)&_ZTVN10__cxxabiv117__class_type_infoE[2],
    "ZapException",
};

/* Keep the in-flight Zap exception object reachable for the conservative GC
 * while it is boxed inside the (malloc'd, unscanned) C++ exception buffer.
 * GC-during-unwind is tracked more thoroughly in ticket #20. */
static void *zap_current_exc;

/* `raise` / built-in faults: begin unwinding carrying the Zap exception object
 * `exc` (currently a ZapStr* message). Never returns normally. */
void zap_panic(void *exc) {
    zap_current_exc = exc;
    void **slot = (void **)__cxa_allocate_exception(sizeof(void *));
    *slot = exc;
    __cxa_throw(slot, (void *)&zap_exception_type_info, NULL);
}

/* Landing-pad entry: recover the Zap exception object from the in-flight C++
 * exception and enter the catch. Paired with `zap_eh_end_catch`. */
void *zap_eh_begin_catch(void *unwind_exc) {
    void **slot = (void **)__cxa_begin_catch(unwind_exc);
    return *slot;
}

void zap_eh_end_catch(void) {
    zap_current_exc = NULL;
    __cxa_end_catch();
}

/* An exception reached the program's top-level handler: report it and exit
 * non-zero (RUNTIME-ABI: "unwinds to the top frame and aborts with a message").
 * A richer stack trace is a follow-up. */
void zap_uncaught(void *exc) {
    ZapStr *msg = (ZapStr *)exc;
    fputs("Uncaught exception: ", stderr);
    if (msg != NULL && msg->len > 0) {
        fwrite(msg->data, 1, (size_t)msg->len, stderr);
    }
    fputc('\n', stderr);
    exit(1);
}

/* ---------------------------------------------------------------------------
 * Decorators & reflection (ticket #16, D-0036, design/0004-decorators.md).
 *
 * Two kinds of support live here, both kept out of the mechanical codegen path:
 *   1. helpers for the compile-time `@derive` decorators — `zap_i64_to_str`
 *      (used by generated `repr` methods) and `zap_str_eq` (used by generated
 *      `eq` methods);
 *   2. the runtime-reflection registry — a flat table of (type, name) pairs
 *      that generated bootstrap code fills at the top of `main`
 *      (`zap_reflect_register`) and that `reflect_has` / `reflect_names` query.
 *
 * The registry is a GC-scanned growable array, so the registered ZapStr*
 * objects stay reachable. Reflection is pay-only-when-used: a program that
 * never calls a `reflect_*` entry point never touches this table.
 * ------------------------------------------------------------------------- */

/* Decimal string for an integer (used by generated `@derive(Repr)` code). */
void *zap_i64_to_str(int64_t v) {
    char tmp[24];
    int n = snprintf(tmp, sizeof(tmp), "%lld", (long long)v);
    return zap_str_new(tmp, (int64_t)n);
}

/* Content equality of two Zap strings (1 = equal), for `@derive(Eq)`. */
int64_t zap_str_eq(void *a, void *b) {
    ZapStr *x = (ZapStr *)a;
    ZapStr *y = (ZapStr *)b;
    if (x == y) {
        return 1;
    }
    if (x == NULL || y == NULL || x->len != y->len) {
        return 0;
    }
    return memcmp(x->data, y->data, (size_t)x->len) == 0 ? 1 : 0;
}

typedef struct {
    ZapStr *type_name;
    ZapStr *deco_name;
} ZapReflectEntry;

typedef struct {
    ZapReflectEntry *items;
    int64_t len;
    int64_t cap;
} ZapReflectTable;

static ZapReflectTable zap_reflect_table;

/* Record that `type_name` carries the annotation/decorator `deco_name`. */
void zap_reflect_register(void *type_name, void *deco_name) {
    ZapReflectTable *t = &zap_reflect_table;
    if (t->len == t->cap) {
        int64_t ncap = t->cap == 0 ? 8 : t->cap * 2;
        ZapReflectEntry *ni =
            (ZapReflectEntry *)GC_MALLOC((size_t)ncap * sizeof(ZapReflectEntry));
        for (int64_t i = 0; i < t->len; i++) {
            ni[i] = t->items[i];
        }
        t->items = ni;
        t->cap = ncap;
    }
    t->items[t->len].type_name = (ZapStr *)type_name;
    t->items[t->len].deco_name = (ZapStr *)deco_name;
    t->len++;
}

/* 1 if `type_name` was registered with `deco_name`, else 0. */
int64_t zap_reflect_has(void *type_name, void *deco_name) {
    ZapReflectTable *t = &zap_reflect_table;
    for (int64_t i = 0; i < t->len; i++) {
        if (zap_str_eq(t->items[i].type_name, type_name) &&
            zap_str_eq(t->items[i].deco_name, deco_name)) {
            return 1;
        }
    }
    return 0;
}

/* The comma-joined names registered for `type_name` (""+ if none), in
 * registration order. */
void *zap_reflect_names(void *type_name) {
    ZapReflectTable *t = &zap_reflect_table;
    ZapStr *out = (ZapStr *)zap_str_new("", 0);
    int first = 1;
    for (int64_t i = 0; i < t->len; i++) {
        if (!zap_str_eq(t->items[i].type_name, type_name)) {
            continue;
        }
        if (!first) {
            out = (ZapStr *)zap_str_concat(out, zap_str_new(", ", 2));
        }
        out = (ZapStr *)zap_str_concat(out, t->items[i].deco_name);
        first = 0;
    }
    return out;
}

/* ---------------------------------------------------------------------------
 * Regex (ticket #23, D-0010): Perl-style `m/.../` and `s/.../.../` backed by
 * PCRE2 (8-bit API). The compiler lowers each literal to one of the entry
 * points below (see crates/ir/src/lower.rs); patterns arrive as `ZapStr*`
 * holding the verbatim pattern text and a flag bitmask.
 *
 * A match is a GC-allocated `ZapMatch` that *owns* copies of every captured
 * group (and a copy of the pattern's name table). Because the captures live in
 * the returned value and not in any global/thread-local state, capture state is
 * coroutine-safe by construction (D-0010) — a goroutine that migrates between
 * OS threads cannot observe another's "last match". It also lets us free the
 * compiled program immediately; a compiled-pattern cache is future work.
 *
 * Flag bits mirror `regex_flag_bits` in lower.rs: 1=i 2=m 4=s 8=x 16=g. */

#define ZAP_RE_CASELESS 1
#define ZAP_RE_MULTILINE 2
#define ZAP_RE_DOTALL 4
#define ZAP_RE_EXTENDED 8
#define ZAP_RE_GLOBAL 16

typedef struct {
    int64_t ngroups;   /* capture groups, including group 0 (whole match) */
    ZapStr **groups;   /* ngroups entries; NULL if the group did not match */
    int64_t nnames;    /* named capture groups */
    ZapStr **names;    /* nnames name strings */
    int64_t *name_num; /* parallel: the group number each name refers to */
} ZapMatch;

/* Non-null test for a reference value. Codegen's `ICmp` only compares integers,
 * so pointer truthiness (`if match:`, `!~`) routes through this. */
int64_t zap_ptr_nonnull(void *p) { return p != NULL ? 1 : 0; }

static uint32_t zap_regex_compile_opts(int64_t flags) {
    uint32_t o = 0;
    if (flags & ZAP_RE_CASELESS) o |= PCRE2_CASELESS;
    if (flags & ZAP_RE_MULTILINE) o |= PCRE2_MULTILINE;
    if (flags & ZAP_RE_DOTALL) o |= PCRE2_DOTALL;
    if (flags & ZAP_RE_EXTENDED) o |= PCRE2_EXTENDED;
    return o;
}

/* Report a regex error and abort (exit 1). Literal-pattern validation is a
 * runtime check for now; a compile-time check would need PCRE2 in the compiler,
 * which must stay host-safe (buildable without LLVM/PCRE2 on the host). */
static void zap_regex_fail(const char *what, int errorcode) {
    PCRE2_UCHAR msg[256];
    pcre2_get_error_message(errorcode, msg, sizeof(msg));
    fprintf(stderr, "regex error: %s: %s\n", what, (const char *)msg);
    exit(1);
}

static pcre2_code *zap_regex_compile(const ZapStr *pattern, int64_t flags) {
    int errorcode;
    PCRE2_SIZE erroroffset;
    pcre2_code *re = pcre2_compile((PCRE2_SPTR)pattern->data, (PCRE2_SIZE)pattern->len,
                                   zap_regex_compile_opts(flags), &errorcode, &erroroffset, NULL);
    if (re == NULL) {
        zap_regex_fail("could not compile pattern", errorcode);
    }
    return re;
}

/* `subject =~ m/pattern/flags` → a ZapMatch* owning its captures, or NULL (no
 * match / falsy). */
void *zap_regex_match(void *subject_, void *pattern_, int64_t flags) {
    ZapStr *subject = (ZapStr *)subject_;
    ZapStr *pattern = (ZapStr *)pattern_;
    pcre2_code *re = zap_regex_compile(pattern, flags);
    pcre2_match_data *md = pcre2_match_data_create_from_pattern(re, NULL);
    int rc = pcre2_match(re, (PCRE2_SPTR)subject->data, (PCRE2_SIZE)subject->len, 0, 0, md, NULL);
    if (rc < 0) {
        /* PCRE2_ERROR_NOMATCH (and any other error) => no match. */
        pcre2_match_data_free(md);
        pcre2_code_free(re);
        return NULL;
    }

    PCRE2_SIZE *ov = pcre2_get_ovector_pointer(md);
    uint32_t capcount = 0;
    pcre2_pattern_info(re, PCRE2_INFO_CAPTURECOUNT, &capcount);
    int64_t ngroups = (int64_t)capcount + 1;
    /* rc == 0 would mean the ovector was too small; sized from the pattern it is
     * not, but guard anyway. */
    int64_t avail = rc > 0 ? (int64_t)rc : ngroups;

    ZapMatch *m = (ZapMatch *)GC_MALLOC(sizeof(ZapMatch));
    m->ngroups = ngroups;
    m->groups = (ZapStr **)GC_MALLOC((size_t)ngroups * sizeof(ZapStr *));
    for (int64_t i = 0; i < ngroups; i++) {
        PCRE2_SIZE s = ov[2 * i];
        PCRE2_SIZE e = ov[2 * i + 1];
        if (i < avail && s != PCRE2_UNSET && e != PCRE2_UNSET && e >= s) {
            m->groups[i] = (ZapStr *)zap_str_new(subject->data + s, (int64_t)(e - s));
        } else {
            m->groups[i] = NULL;
        }
    }

    /* Copy the name table so lookups work after the compiled program is freed.
     * Each entry is [group-number (2 bytes, big-endian)][name][NUL]. */
    uint32_t namecount = 0, nameentrysize = 0;
    PCRE2_SPTR nametable = NULL;
    pcre2_pattern_info(re, PCRE2_INFO_NAMECOUNT, &namecount);
    pcre2_pattern_info(re, PCRE2_INFO_NAMEENTRYSIZE, &nameentrysize);
    pcre2_pattern_info(re, PCRE2_INFO_NAMETABLE, &nametable);
    m->nnames = (int64_t)namecount;
    if (namecount > 0) {
        m->names = (ZapStr **)GC_MALLOC((size_t)namecount * sizeof(ZapStr *));
        m->name_num = (int64_t *)GC_MALLOC((size_t)namecount * sizeof(int64_t));
        for (uint32_t i = 0; i < namecount; i++) {
            PCRE2_SPTR entry = nametable + (size_t)i * nameentrysize;
            int gnum = (entry[0] << 8) | entry[1];
            const char *nm = (const char *)(entry + 2);
            m->names[i] = (ZapStr *)zap_str_new(nm, (int64_t)strlen(nm));
            m->name_num[i] = gnum;
        }
    } else {
        m->names = NULL;
        m->name_num = NULL;
    }

    pcre2_match_data_free(md);
    pcre2_code_free(re);
    return m;
}

/* `m.group(idx)` — positional capture; 0 is the whole match. Out-of-range or
 * non-participating groups yield the empty string. */
void *zap_regex_group(void *m_, int64_t idx) {
    ZapMatch *m = (ZapMatch *)m_;
    if (m == NULL || idx < 0 || idx >= m->ngroups || m->groups[idx] == NULL) {
        return zap_str_new("", 0);
    }
    return m->groups[idx];
}

/* `m.group("name")` — named capture; unknown or non-participating names yield
 * the empty string. */
void *zap_regex_group_name(void *m_, void *name_) {
    ZapMatch *m = (ZapMatch *)m_;
    ZapStr *name = (ZapStr *)name_;
    if (m != NULL) {
        for (int64_t i = 0; i < m->nnames; i++) {
            if (zap_str_eq(m->names[i], name)) {
                int64_t g = m->name_num[i];
                if (g >= 0 && g < m->ngroups && m->groups[g] != NULL) {
                    return m->groups[g];
                }
                break;
            }
        }
    }
    return zap_str_new("", 0);
}

/* `subject =~ s/pattern/replacement/flags` → a new string (subject unchanged).
 * `g` replaces all occurrences, else the first. Replacement uses PCRE2 syntax
 * for group references (`$1`, `${name}`). */
void *zap_regex_subst(void *subject_, void *pattern_, void *replacement_, int64_t flags) {
    ZapStr *subject = (ZapStr *)subject_;
    ZapStr *pattern = (ZapStr *)pattern_;
    ZapStr *replacement = (ZapStr *)replacement_;
    pcre2_code *re = zap_regex_compile(pattern, flags);

    uint32_t opts = PCRE2_SUBSTITUTE_OVERFLOW_LENGTH;
    if (flags & ZAP_RE_GLOBAL) opts |= PCRE2_SUBSTITUTE_GLOBAL;

    /* OVERFLOW_LENGTH makes a too-small buffer set *outlen to the needed size
     * (including the NUL) and return PCRE2_ERROR_NOMEMORY, so we size, grow,
     * and retry at most once. On success *outlen is the length excluding NUL. */
    PCRE2_SIZE outlen = (PCRE2_SIZE)subject->len + 256;
    PCRE2_UCHAR *buf = (PCRE2_UCHAR *)GC_MALLOC_ATOMIC((size_t)outlen);
    int rc = pcre2_substitute(re, (PCRE2_SPTR)subject->data, (PCRE2_SIZE)subject->len, 0, opts,
                              NULL, NULL, (PCRE2_SPTR)replacement->data,
                              (PCRE2_SIZE)replacement->len, buf, &outlen);
    if (rc == PCRE2_ERROR_NOMEMORY) {
        buf = (PCRE2_UCHAR *)GC_MALLOC_ATOMIC((size_t)outlen);
        rc = pcre2_substitute(re, (PCRE2_SPTR)subject->data, (PCRE2_SIZE)subject->len, 0, opts,
                              NULL, NULL, (PCRE2_SPTR)replacement->data,
                              (PCRE2_SIZE)replacement->len, buf, &outlen);
    }
    pcre2_code_free(re);
    if (rc < 0) {
        zap_regex_fail("substitution failed", rc);
    }
    return zap_str_new((const char *)buf, (int64_t)outlen);
}

/* ---------------------------------------------------------------------------
 * Python interop (ticket #27, D-0012, design/0003-python-interop.md): embed
 * CPython and marshal values across the Zap <-> Python boundary. The compiler
 * lowers `import python`, attribute/call dispatch, and conversions to the
 * `zap_py_*` entry points below (see crates/ir/src/lower.rs).
 *
 * Representation & GC bridging: a Python value held by Zap is an opaque,
 * GC-managed handle `ZapPy` that OWNS one CPython reference. Capture takes a new
 * reference (the C-API calls here return new references, handed straight to
 * `zap_py_wrap`); a Boehm finalizer DECREFs when the wrapper is collected. The
 * wrapper is GC-scanned, and the ZapList argument vectors that carry handles are
 * GC_MALLOC'd, so a live Zap handle keeps its PyObject alive across collection.
 *
 * GIL: every entry point acquires the GIL (PyGILState_Ensure) around its C-API
 * work and releases it before returning. With the single-threaded runtime of
 * this increment the main thread simply re-acquires it; GIL contention vs.
 * goroutine parallelism (#25) is a documented limitation (design open issue).
 *
 * Exceptions: a Python error is translated to a Zap exception — the error is
 * formatted as "<type>: <message>" into a ZapStr and raised with `zap_panic`,
 * so it propagates and is catchable with `try`/`except` (D-0007, ticket #22).
 *
 * Follow-ups (documented in the PR): list/dict/ndarray marshaling (the design's
 * container rows), keyword arguments, real `PyObject_IsTrue` truthiness, and a
 * thread-safe interpreter init for the goroutine runtime. */

typedef struct {
    PyObject *obj; /* owned reference; NULL once finalized */
} ZapPy;

/* Initialize the interpreter exactly once (single-threaded this increment). */
static void zap_py_ensure_init(void) {
    if (!Py_IsInitialized()) {
        Py_InitializeEx(0); /* 0 = do not install signal handlers */
    }
}

/* Boehm finalizer: drop the owned CPython reference when the wrapper dies. */
static void zap_py_finalize(void *obj, void *client_data) {
    (void)client_data;
    ZapPy *h = (ZapPy *)obj;
    if (h->obj != NULL) {
        PyGILState_STATE g = PyGILState_Ensure();
        Py_DECREF(h->obj);
        PyGILState_Release(g);
        h->obj = NULL;
    }
}

/* Wrap a *new* reference `o` (ownership transferred to the handle). */
static ZapPy *zap_py_wrap(PyObject *o) {
    ZapPy *h = (ZapPy *)GC_MALLOC(sizeof(ZapPy));
    h->obj = o;
    GC_register_finalizer(h, zap_py_finalize, NULL, NULL, NULL);
    return h;
}

/* NUL-terminated copy of a ZapStr (for the C-string CPython APIs). */
static char *zap_py_cstr(const ZapStr *s) {
    char *buf = (char *)GC_MALLOC_ATOMIC((size_t)s->len + 1);
    if (s->len > 0) {
        memcpy(buf, s->data, (size_t)s->len);
    }
    buf[s->len] = '\0';
    return buf;
}

/* Translate the current Python error to a Zap exception and raise it. Releases
 * the GIL first (the caller passes its gstate), then calls `zap_panic`, which
 * unwinds and never returns. */
static void zap_py_raise(PyGILState_STATE g) {
    PyObject *type = NULL, *value = NULL, *tb = NULL;
    PyErr_Fetch(&type, &value, &tb);
    PyErr_NormalizeException(&type, &value, &tb);
    const char *tname =
        (type != NULL) ? ((PyTypeObject *)type)->tp_name : "PythonError";
    ZapStr *msg = (ZapStr *)zap_str_new(tname, (int64_t)strlen(tname));
    if (value != NULL) {
        PyObject *s = PyObject_Str(value);
        if (s != NULL) {
            Py_ssize_t n = 0;
            const char *u = PyUnicode_AsUTF8AndSize(s, &n);
            ZapStr *sep = (ZapStr *)zap_str_new(": ", 2);
            ZapStr *body = (ZapStr *)zap_str_new(u ? u : "", (int64_t)(u ? n : 0));
            msg = (ZapStr *)zap_str_concat(zap_str_concat(msg, sep), body);
            Py_DECREF(s);
        }
    }
    Py_XDECREF(type);
    Py_XDECREF(value);
    Py_XDECREF(tb);
    PyGILState_Release(g);
    zap_panic(msg);
}

/* `import python "module"` → a module handle. */
void *zap_py_import(void *name_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    char *name = zap_py_cstr((ZapStr *)name_);
    PyObject *m = PyImport_ImportModule(name);
    if (m == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(m);
    PyGILState_Release(g);
    return out;
}

/* `obj.attr` on a Python handle → the attribute value (another handle). */
void *zap_py_getattr(void *obj_, void *name_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapPy *obj = (ZapPy *)obj_;
    char *name = zap_py_cstr((ZapStr *)name_);
    PyObject *a = PyObject_GetAttrString(obj->obj, name);
    if (a == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(a);
    PyGILState_Release(g);
    return out;
}

/* Call a Python callable with positional args collected in a ZapList of ZapPy*
 * handle words (built by the lowering). Returns the result as a handle. */
void *zap_py_call(void *callable_, void *arglist_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapPy *callable = (ZapPy *)callable_;
    ZapList *argl = (ZapList *)arglist_;
    int64_t n = (argl != NULL) ? argl->len : 0;
    PyObject *tuple = PyTuple_New((Py_ssize_t)n);
    if (tuple == NULL) {
        zap_py_raise(g);
    }
    for (int64_t i = 0; i < n; i++) {
        ZapPy *h = (ZapPy *)(intptr_t)argl->data[i];
        PyObject *o = h->obj;
        Py_INCREF(o); /* PyTuple_SET_ITEM steals the reference */
        PyTuple_SET_ITEM(tuple, (Py_ssize_t)i, o);
    }
    PyObject *r = PyObject_CallObject(callable->obj, tuple);
    Py_DECREF(tuple);
    if (r == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(r);
    PyGILState_Release(g);
    return out;
}

/* --- marshaling: Zap scalar/str -> Python (the handle owns the new ref). --- */
void *zap_py_from_i64(int64_t v) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *o = PyLong_FromLongLong((long long)v);
    if (o == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(o);
    PyGILState_Release(g);
    return out;
}
void *zap_py_from_f64(double v) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *o = PyFloat_FromDouble(v);
    if (o == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(o);
    PyGILState_Release(g);
    return out;
}
void *zap_py_from_bool(int64_t v) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *o = PyBool_FromLong((long)v); /* new ref to Py_True/Py_False */
    ZapPy *out = zap_py_wrap(o);
    PyGILState_Release(g);
    return out;
}
void *zap_py_from_str(void *s_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapStr *s = (ZapStr *)s_;
    PyObject *o = PyUnicode_FromStringAndSize(s->data, (Py_ssize_t)s->len);
    if (o == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(o);
    PyGILState_Release(g);
    return out;
}

/* --- marshaling: Python -> Zap scalar/str. --- */
int64_t zap_py_to_i64(void *obj_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapPy *obj = (ZapPy *)obj_;
    PyObject *n = PyNumber_Long(obj->obj); /* accepts int and float */
    if (n == NULL) {
        zap_py_raise(g);
    }
    long long v = PyLong_AsLongLong(n);
    Py_DECREF(n);
    if (v == -1 && PyErr_Occurred()) {
        zap_py_raise(g);
    }
    PyGILState_Release(g);
    return (int64_t)v;
}
double zap_py_to_f64(void *obj_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapPy *obj = (ZapPy *)obj_;
    double v = PyFloat_AsDouble(obj->obj); /* accepts int and float */
    if (v == -1.0 && PyErr_Occurred()) {
        zap_py_raise(g);
    }
    PyGILState_Release(g);
    return v;
}

/* `str(obj)` as a Zap string (used by `print` of a Python handle). */
void *zap_py_str(void *obj_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapPy *obj = (ZapPy *)obj_;
    PyObject *s = PyObject_Str(obj->obj);
    if (s == NULL) {
        zap_py_raise(g);
    }
    Py_ssize_t n = 0;
    const char *u = PyUnicode_AsUTF8AndSize(s, &n);
    ZapStr *out = (ZapStr *)zap_str_new(u ? u : "", (int64_t)(u ? n : 0));
    Py_DECREF(s);
    PyGILState_Release(g);
    return out;
}

/* ===========================================================================
 * Goroutines + channels (ticket #25, D-0013/D-0031/D-0033/D-0034,
 * design/0005-concurrency.md).
 *
 * This is a correct, shippable milestone of the locked CSP surface, backed by
 * OS threads rather than a user-space M:N scheduler:
 *
 *   - `spawn f(args)` starts one GC-registered pthread (`GC_pthread_create`)
 *     running `f` on the eagerly-evaluated argument closure. Fire-and-forget:
 *     no handle. The implicit root scope joins every spawned thread at program
 *     exit (`zap_rt_join_all`, emitted at `main`'s return), so a bare `spawn`
 *     is never truly detached (D-0013/D-0033).
 *   - Channels are `channel[T]()` (unbuffered rendezvous) and `channel[T](cap)`
 *     (buffered), with `<-` send/receive and `close`, blocking on a
 *     mutex/condvar (D-0031). Elements are uniform 8-byte words; the typed
 *     entry points reinterpret them as `int64` or pointer.
 *   - Channel/close operations are the happens-before synchronization edges the
 *     DRF-SC model needs (D-0034); the mutex/condvar provide them directly.
 *
 * Because every goroutine runs on a GC-registered OS thread, the collector
 * scans each one's stack and registers, so live objects reachable only from a
 * running goroutine stay alive with no extra GC machinery (the property
 * design/0005 relies on for the green-thread stacks). GC_THREADS (defined above
 * <gc.h>) and GC_pthread_create provide that registration.
 *
 * DEFERRED (language-visible semantics unchanged; see the PR / OPEN-QUESTIONS):
 * the user-space work-stealing M:N scheduler with green-thread stacks, the
 * `scope:` block and cooperative cancellation, `select`, the comma-ok receive
 * `v, ok = <-ch` and `for v in ch:`, and the `sync` module
 * (Mutex/WaitGroup/Once/Atomic). The runtime ABI here is additive, so these
 * drop in without changing the surface already compiled.
 * =========================================================================== */

typedef struct {
    pthread_mutex_t mu;
    pthread_cond_t not_full;  /* buffered: space available; unbuffered: slot emptied */
    pthread_cond_t not_empty; /* buffered: item available; unbuffered: slot filled   */
    pthread_cond_t drained;   /* unbuffered: the handed-off value was taken          */
    pthread_mutex_t send_mu;  /* unbuffered: serializes senders through the slot     */
    uint64_t *buf;            /* `cap` element words (buffered); NULL when unbuffered */
    int64_t cap;              /* buffer capacity; 0 => unbuffered rendezvous          */
    int64_t count;            /* items currently buffered                            */
    int64_t head;             /* next slot to read                                   */
    int64_t tail;             /* next slot to write                                  */
    int closed;
    uint64_t u_slot; /* unbuffered rendezvous hand-off slot                 */
    int u_has;       /* a sender has deposited a value awaiting a receiver  */
} ZapChan;

/* `channel[T]()` / `channel[T](cap)`. The buffer is GC_MALLOC (pointer-scanned,
 * not atomic) so a pointer-element channel keeps its queued references alive. */
void *zap_chan_new(int64_t cap) {
    ZapChan *c = (ZapChan *)GC_MALLOC(sizeof(ZapChan));
    pthread_mutex_init(&c->mu, NULL);
    pthread_cond_init(&c->not_full, NULL);
    pthread_cond_init(&c->not_empty, NULL);
    pthread_cond_init(&c->drained, NULL);
    pthread_mutex_init(&c->send_mu, NULL);
    c->cap = cap > 0 ? cap : 0;
    c->buf = c->cap > 0
                 ? (uint64_t *)GC_MALLOC((size_t)c->cap * sizeof(uint64_t))
                 : NULL;
    c->count = 0;
    c->head = 0;
    c->tail = 0;
    c->closed = 0;
    c->u_slot = 0;
    c->u_has = 0;
    return c;
}

/* `send on a closed channel` is a programming error that raises (D-0031).
 * Releases the channel lock (and, when given, the unbuffered send lock) before
 * raising so the channel is left in a consistent, unlocked state. */
static void chan_send_closed(ZapChan *c, pthread_mutex_t *also) {
    pthread_mutex_unlock(&c->mu);
    if (also) {
        pthread_mutex_unlock(also);
    }
    zap_panic(zap_str_new("send on closed channel", 22));
}

static void chan_send_word(ZapChan *c, uint64_t v) {
    if (c->cap > 0) {
        pthread_mutex_lock(&c->mu);
        while (c->count == c->cap && !c->closed) {
            pthread_cond_wait(&c->not_full, &c->mu);
        }
        if (c->closed) {
            chan_send_closed(c, NULL);
            return;
        }
        c->buf[c->tail] = v;
        c->tail = (c->tail + 1) % c->cap;
        c->count++;
        pthread_cond_signal(&c->not_empty);
        pthread_mutex_unlock(&c->mu);
    } else {
        /* Rendezvous: only one sender occupies the hand-off slot at a time
         * (`send_mu`), so the slot's `u_has` flag unambiguously tracks *this*
         * sender's value — deposit it, wake a receiver, then block until the
         * receiver takes it, so the send does not complete until a receive does
         * (true unbuffered semantics, correct for many concurrent senders). */
        pthread_mutex_lock(&c->send_mu);
        pthread_mutex_lock(&c->mu);
        if (c->closed) {
            chan_send_closed(c, &c->send_mu);
            return;
        }
        c->u_slot = v;
        c->u_has = 1;
        pthread_cond_signal(&c->not_empty);
        while (c->u_has && !c->closed) {
            pthread_cond_wait(&c->drained, &c->mu);
        }
        if (c->u_has && c->closed) {
            c->u_has = 0;
            chan_send_closed(c, &c->send_mu);
            return;
        }
        pthread_mutex_unlock(&c->mu);
        pthread_mutex_unlock(&c->send_mu);
    }
}

/* Receive one word. `*ok` is set to 1 on a real value, 0 when the channel is
 * closed and drained (the zero-value case the comma-ok form will surface). */
static uint64_t chan_recv_word(ZapChan *c, int *ok) {
    pthread_mutex_lock(&c->mu);
    if (c->cap > 0) {
        while (c->count == 0 && !c->closed) {
            pthread_cond_wait(&c->not_empty, &c->mu);
        }
        if (c->count == 0 && c->closed) {
            pthread_mutex_unlock(&c->mu);
            if (ok) {
                *ok = 0;
            }
            return 0;
        }
        uint64_t v = c->buf[c->head];
        c->head = (c->head + 1) % c->cap;
        c->count--;
        pthread_cond_signal(&c->not_full);
        pthread_mutex_unlock(&c->mu);
        if (ok) {
            *ok = 1;
        }
        return v;
    } else {
        while (!c->u_has && !c->closed) {
            pthread_cond_wait(&c->not_empty, &c->mu);
        }
        if (!c->u_has && c->closed) {
            pthread_mutex_unlock(&c->mu);
            if (ok) {
                *ok = 0;
            }
            return 0;
        }
        uint64_t v = c->u_slot;
        c->u_has = 0;
        pthread_cond_signal(&c->drained);  /* wake the matched sender */
        pthread_cond_signal(&c->not_full); /* let another sender deposit */
        pthread_mutex_unlock(&c->mu);
        if (ok) {
            *ok = 1;
        }
        return v;
    }
}

void zap_chan_send_i64(void *ch, int64_t v) {
    chan_send_word((ZapChan *)ch, (uint64_t)v);
}
void zap_chan_send_ptr(void *ch, void *v) {
    chan_send_word((ZapChan *)ch, (uint64_t)(uintptr_t)v);
}
int64_t zap_chan_recv_i64(void *ch) {
    int ok;
    return (int64_t)chan_recv_word((ZapChan *)ch, &ok);
}
void *zap_chan_recv_ptr(void *ch) {
    int ok;
    return (void *)(uintptr_t)chan_recv_word((ZapChan *)ch, &ok);
}

void zap_chan_close(void *ch) {
    ZapChan *c = (ZapChan *)ch;
    pthread_mutex_lock(&c->mu);
    c->closed = 1;
    pthread_cond_broadcast(&c->not_empty);
    pthread_cond_broadcast(&c->not_full);
    pthread_cond_broadcast(&c->drained);
    pthread_mutex_unlock(&c->mu);
}

/* --- goroutine spawn + root-scope join -------------------------------------
 *
 * The registry tracks every spawned thread so the root scope can join them all
 * at program exit. It grows under its own lock; goroutines that spawn further
 * goroutines append to the same registry, and `zap_rt_join_all` keeps joining
 * until the list is drained (so the whole coroutine tree completes). */
static pthread_mutex_t zap_threads_mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_t *zap_threads = NULL;
static int64_t zap_threads_len = 0;
static int64_t zap_threads_cap = 0;
static int64_t zap_threads_joined = 0;

/* `zap_spawn(trampoline, closure)`: start a GC-registered thread running
 * `trampoline(closure)`. `GC_pthread_create` registers the new thread with the
 * collector so its stack and registers are scanned (keeping objects reachable
 * only from the running goroutine alive). */
void zap_spawn(void *fn_, void *arg) {
    void *(*fn)(void *) = (void *(*)(void *))fn_;
    pthread_t t;
    int rc = GC_pthread_create(&t, NULL, fn, arg);
    if (rc != 0) {
        fprintf(stderr, "zap: failed to spawn coroutine (pthread_create: %d)\n", rc);
        abort();
    }
    pthread_mutex_lock(&zap_threads_mu);
    if (zap_threads_len == zap_threads_cap) {
        int64_t ncap = zap_threads_cap ? zap_threads_cap * 2 : 8;
        pthread_t *n =
            (pthread_t *)GC_MALLOC_ATOMIC((size_t)ncap * sizeof(pthread_t));
        if (zap_threads) {
            memcpy(n, zap_threads, (size_t)zap_threads_len * sizeof(pthread_t));
        }
        zap_threads = n;
        zap_threads_cap = ncap;
    }
    zap_threads[zap_threads_len++] = t;
    pthread_mutex_unlock(&zap_threads_mu);
}

/* Join every spawned coroutine (the implicit root scope's join at program
 * exit). Re-reads the length each iteration so coroutines spawned by other
 * coroutines are joined too. */
void zap_rt_join_all(void) {
    for (;;) {
        pthread_mutex_lock(&zap_threads_mu);
        if (zap_threads_joined >= zap_threads_len) {
            pthread_mutex_unlock(&zap_threads_mu);
            return;
        }
        pthread_t t = zap_threads[zap_threads_joined++];
        pthread_mutex_unlock(&zap_threads_mu);
        GC_pthread_join(t, NULL);
    }
}
