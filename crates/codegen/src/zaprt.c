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
#include <gc.h>
#include <math.h>
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
