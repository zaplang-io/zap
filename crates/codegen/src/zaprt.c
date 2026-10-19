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

#ifndef ZAP_GC_IMMIX
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
#else
/* Precise Immix GC build (ticket #48, design/0008-precise-gc.md): no Boehm. The
 * scheduler's conservative-GC hooks below (thread registration, stack-bottom
 * tracking, the alloc lock) become inert shims — the precise collector finds
 * roots via the per-coroutine shadow stack, so it needs neither stack scanning
 * nor bdwgc's pthread wrappers. The collector itself is #included at the bottom
 * of this file. */
#include <stdint.h>
#define GC_pthread_create pthread_create
#define GC_pthread_join pthread_join
#define GC_call_with_alloc_lock(fn, arg) ((fn)(arg))
struct GC_stack_base {
    void *mem_base;
};
static void *GC_get_my_stackbottom(struct GC_stack_base *sb) {
    sb->mem_base = NULL;
    return NULL;
}
static void GC_set_stackbottom(void *h, struct GC_stack_base *sb) {
    (void)h;
    (void)sb;
}
#endif
#include <math.h>
#include <pthread.h>
#include <stdint.h>
/* ucontext (getcontext/makecontext/swapcontext) backs the M:N scheduler's
 * green-thread context switch (ticket #47). Available on the Linux x86_64 dev
 * container; see the scheduler block below for the arch assumption. */
#include <ucontext.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>   /* clock_gettime for the scheduler idle wait (#47) */
#include <unistd.h> /* sysconf(_SC_NPROCESSORS_ONLN) for the M count (#47) */

/* PCRE2, 8-bit code units (ticket #23, D-0010). Must be defined before the
 * header so the `pcre2_*` names resolve to the 8-bit API. Linked with
 * `-lpcre2-8` (see build_executable). */
#define PCRE2_CODE_UNIT_WIDTH 8
#include <pcre2.h>

/* ===========================================================================
 * GC abstraction (ticket #48). The runtime and codegen allocate through a
 * small, GC-agnostic surface so the Boehm (default) and precise Immix
 * (`gc-immix`) collectors are interchangeable behind the same call sites:
 *
 *   zap_alloc_desc(desc) — a fixed-size object described by `desc` (codegen
 *       structs, and the runtime's own collection headers).
 *   zap_obj(desc)        — runtime-internal alias of zap_alloc_desc.
 *   zap_buf(n)           — an n-byte leaf buffer with no pointers (string data,
 *       numeric array data, and container element/entry buffers, whose pointer
 *       interiors the owning container traces precisely under Immix).
 *   zap_ptrbuf(n)        — an n-byte buffer that MAY hold pointer words. Under
 *       Boehm it is conservatively scanned; under Immix the owning container
 *       traces it precisely, so it is allocated as a leaf.
 *
 * The TypeDesc is the shared layout contract: codegen emits one per user struct
 * (kind PRECISE with the pointer field byte-offsets); the runtime defines the
 * CONTAINER descriptors below. Both collectors read `.size`; only Immix reads
 * the trace fields. The kind codes are shared so both builds compile. */
typedef struct ZapTypeDesc {
    int64_t size;               /* payload bytes for a fixed-size object        */
    int32_t kind;               /* ZAP_TD_* (see below)                         */
    int32_t n_ptrs;             /* PRECISE: number of pointer offsets           */
    const int32_t *ptr_offsets; /* PRECISE: pointer field byte offsets in payload */
} ZapTypeDesc;

enum {
    ZAP_TD_PRECISE = 0,
    ZAP_TD_ATOMIC = 1,
    ZAP_TD_LIST = 2,
    ZAP_TD_DICT = 3,
    ZAP_TD_SET = 4,
    ZAP_TD_HEAP = 5,
    ZAP_TD_CHAN = 6,
    ZAP_TD_ARRAY = 7,
    ZAP_TD_GSTACK = 8,
    ZAP_TD_MATCH = 9,  /* ZapMatch (regex): groups/names buffers of ZapStr*     */
    ZAP_TD_SCOPE = 10  /* Scope: excs buffer of ZapStr* pending failures        */
};

/* Runtime-internal container descriptors (defined at the bottom of this file). */
extern const ZapTypeDesc zap_desc_atomic;
extern const ZapTypeDesc zap_desc_list;
extern const ZapTypeDesc zap_desc_dict;
extern const ZapTypeDesc zap_desc_set;
extern const ZapTypeDesc zap_desc_heap;
extern const ZapTypeDesc zap_desc_chan;
extern const ZapTypeDesc zap_desc_array;
extern const ZapTypeDesc zap_desc_scope;
extern const ZapTypeDesc zap_desc_gstack;
extern const ZapTypeDesc zap_desc_strobj;  /* ZapStr header (one ptr: data)     */
extern const ZapTypeDesc zap_desc_matchobj;
extern const ZapTypeDesc zap_desc_pyobj;
extern const ZapTypeDesc zap_desc_gobj;    /* scheduler G (precise)             */
extern const ZapTypeDesc zap_desc_ptrs;    /* a generic array-of-pointers buffer */

#ifdef ZAP_GC_IMMIX
/* Precise collector entry points (defined in zapgc.inc.c, #included below). */
void *zap_gc_alloc(const ZapTypeDesc *desc);
void *zap_gc_alloc_atomic(int64_t size);
void *zap_gc_alloc_var(const ZapTypeDesc *desc, int64_t size);
void zap_safepoint(void);
void zap_gc_add_root(void **slot);
void zap_gc_add_root_fn(void (*fn)(void));
void zap_gc_mark(void *payload);
void zap_gc_collect(void);
extern __thread void *zap_shadow_head;
extern void *zap_main_shadow_head;
static void zap_enter_blocking(void); /* fwd (defined in zapgc.inc.c)           */
static void zap_leave_blocking(void);
static void zap_mutator_register(void);

static inline void *zap_alloc_desc(const ZapTypeDesc *d) { return zap_gc_alloc(d); }
static inline void *zap_obj(const ZapTypeDesc *d) { return zap_gc_alloc(d); }
static inline void *zap_buf(int64_t n) { return zap_gc_alloc_atomic(n); }
static inline void *zap_ptrbuf(int64_t n) { return zap_gc_alloc_atomic(n); }
/* Scheduler infrastructure arrays (g_ps/g_ms/g_all) are read by the collector
 * directly via C globals, not traced as Zap objects, so they use plain malloc
 * and live for the program's lifetime. */
#define ZAP_INFRA_ALLOC(n) malloc(n)
#define GC_register_finalizer(obj, fn, cd, ofn, ocd)                           \
    ((void)(obj), (void)(fn), (void)(cd), (void)(ofn), (void)(ocd))
#else
/* Boehm: a safepoint is a no-op, roots are found conservatively, and every
 * allocation routes to GC_MALLOC / GC_MALLOC_ATOMIC. `desc` carries the size. */
#define zap_safepoint() ((void)0)
#define zap_enter_blocking() ((void)0)
#define zap_leave_blocking() ((void)0)
#define zap_mutator_register() ((void)0)
#define zap_gc_add_root(slot) ((void)(slot))
#define zap_gc_add_root_fn(fn) ((void)(fn))
#define zap_gc_mark(p) ((void)(p))
static inline void *zap_alloc_desc(const ZapTypeDesc *d) {
    return GC_MALLOC((size_t)d->size);
}
static inline void *zap_obj(const ZapTypeDesc *d) { return GC_MALLOC((size_t)d->size); }
static inline void *zap_buf(int64_t n) { return GC_MALLOC_ATOMIC((size_t)n); }
static inline void *zap_ptrbuf(int64_t n) { return GC_MALLOC((size_t)n); }
#define ZAP_INFRA_ALLOC(n) GC_MALLOC(n)
#endif

typedef struct {
    int64_t len;
    const char *data;
} ZapStr;

/* Runtime startup: initialize the collector before any Zap code runs. Under
 * Boehm this is `GC_INIT`; under Immix it registers the main OS thread (which
 * runs top-level Zap) as a mutator for the stop-the-world handshake. */
__attribute__((constructor)) static void zap_gc_init(void) {
#ifdef ZAP_GC_IMMIX
    zap_mutator_register();
#else
    GC_INIT();
#endif
}

/* Legacy untyped allocation. Retained for ABI/source stability; under the
 * precise collector every real allocation is descriptor-typed (codegen emits a
 * descriptor for structs and closures), so this is used only for the Boehm
 * build. */
void *zap_alloc(int64_t size) {
#ifdef ZAP_GC_IMMIX
    /* No descriptor: treat as a pointer-free leaf. Codegen does not emit raw
     * zap_alloc under gc-immix (it calls zap_alloc_desc), so this is a safety
     * net rather than a hot path. */
    return zap_gc_alloc_atomic(size);
#else
    return GC_MALLOC((size_t)size);
#endif
}

/* ---------------------------------------------------------------------------
 * Uniform GC control surface (ticket #77 signals, backend-agnostic): one symbol
 * each backend implements, so a SIGUSR1 "force GC + memory stats" handler (and
 * any future runtime introspection) calls it without `#ifdef`-ing per backend.
 *   - zap_gc_collect(): force a full collection now.
 *   - zap_gc_stats(heap_bytes, live_bytes): current mapped-heap bytes and the
 *     live/used bytes as of the last collection (best-effort per backend).
 * The Immix implementations live in zapgc.inc.c (included below); the Boehm
 * ones wrap bdwgc here. */
#ifndef ZAP_GC_IMMIX
void zap_gc_collect(void) { GC_gcollect(); }
void zap_gc_stats(int64_t *heap_bytes, int64_t *live_bytes) {
    size_t heap = GC_get_heap_size();
    size_t freeb = GC_get_free_bytes();
    if (heap_bytes) {
        *heap_bytes = (int64_t)heap;
    }
    if (live_bytes) {
        *live_bytes = (int64_t)(heap > freeb ? heap - freeb : 0);
    }
}
#endif

/* Descriptor-typed allocation the compiler calls for struct / closure objects
 * (ticket #48). Exposed with C linkage so codegen can declare and call it. */
void *zap_alloc_typed(const ZapTypeDesc *desc) { return zap_alloc_desc(desc); }

void *zap_str_new(const char *data, int64_t len) {
    ZapStr *s = (ZapStr *)zap_obj(&zap_desc_strobj);
    char *buf = (char *)zap_buf((size_t)len);
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
    char *buf = (char *)zap_buf((size_t)n);
    memcpy(buf, x->data, (size_t)x->len);
    memcpy(buf + x->len, y->data, (size_t)y->len);
    ZapStr *s = (ZapStr *)zap_obj(&zap_desc_strobj);
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
    uint8_t elem_ptr; /* 1 iff elements are GC pointers (set by _ptr push) */
} ZapList;

void *zap_list_new(void) {
    ZapList *l = (ZapList *)zap_obj(&zap_desc_list);
    l->len = 0;
    l->cap = 0;
    l->data = NULL;
    return l;
}

static void zap_list_grow(ZapList *l) {
    int64_t cap = l->cap < 4 ? 4 : l->cap * 2;
    int64_t *data = (int64_t *)zap_ptrbuf((int64_t)cap * (int64_t)sizeof(int64_t));
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
    ((ZapList *)l)->elem_ptr = 1; /* precise GC: elements are pointers (#48) */
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
    ((ZapList *)l)->elem_ptr = 1;
    (void)zap_list_at((ZapList *)l, i); /* bounds-check */
    ((ZapList *)l)->data[i] = (int64_t)(intptr_t)v;
}
int64_t zap_list_len(void *l) { return ((ZapList *)l)->len; }

/* dict[K, V]: open-addressing hash table with linear probing.
 * Key kind (int64 vs str) is passed per call; a dict is homogeneous, so this is
 * stable across a given dict's lifetime. */
/* KEY_FN (operator overloading, D-0047): a user-struct key, hashed/compared
 * through the type's own `__hash__`/`__eq__`, which the dict/set stores as
 * function pointers. `__hash__(self) -> i64` and `__eq__(a, b) -> bool` match the
 * ZIR-generated signatures (an LLVM `i1` return maps to C `_Bool`). */
enum { KEY_I64 = 0, KEY_STR = 1, KEY_FN = 2 };
enum { SLOT_EMPTY = 0, SLOT_FULL = 1, SLOT_TOMB = 2 };

typedef int64_t (*ZapHashFn)(void *self);
typedef _Bool (*ZapEqFn)(void *a, void *b);

typedef struct {
    uint8_t state;
    int64_t key; /* key word: int64, or ZapStr/struct pointer bits */
    int64_t val; /* value word */
} ZapEntry;

typedef struct {
    int64_t len;
    int64_t cap; /* number of slots; always a power of two or 0 */
    ZapEntry *entries;
    void *hashfn; /* struct keys only (KEY_FN): the type's __hash__ / __eq__ */
    void *eqfn;
    uint8_t key_ptr; /* keys are GC pointers (str/struct) */
    uint8_t val_ptr; /* values are GC pointers */
} ZapDict;

void *zap_dict_new(void) {
    ZapDict *d = (ZapDict *)zap_obj(&zap_desc_dict);
    d->len = 0;
    d->cap = 0;
    d->entries = NULL;
    d->hashfn = NULL;
    d->eqfn = NULL;
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

static uint64_t zap_key_hash(int64_t key, int keykind, void *hashfn) {
    if (keykind == KEY_FN) {
        /* User struct key: hash via the type's __hash__ (D-0047). */
        return (uint64_t)((ZapHashFn)hashfn)((void *)(intptr_t)key);
    }
    return keykind == KEY_STR ? zap_hash_str((const ZapStr *)(intptr_t)key)
                              : zap_hash_i64(key);
}

static int zap_key_eq(int64_t a, int64_t b, int keykind, void *eqfn) {
    if (keykind == KEY_FN) {
        /* User struct key: compare via the type's __eq__ (D-0047). */
        return ((ZapEqFn)eqfn)((void *)(intptr_t)a, (void *)(intptr_t)b) ? 1 : 0;
    }
    if (keykind != KEY_STR) {
        return a == b;
    }
    const ZapStr *x = (const ZapStr *)(intptr_t)a;
    const ZapStr *y = (const ZapStr *)(intptr_t)b;
    return x->len == y->len && memcmp(x->data, y->data, (size_t)x->len) == 0;
}

/* Hashing helpers for @derive(Hash)/@derive(Ord) generated dunders (operator
 * overloading, D-0047). `__hash__` combines each field's hash with
 * `zap_hash_combine`; `__cmp__` compares string fields with `zap_str_cmp`. */
int64_t zap_i64_hash(int64_t k) { return (int64_t)zap_hash_i64(k); }
int64_t zap_str_hash(void *s) { return (int64_t)zap_hash_str((const ZapStr *)s); }
int64_t zap_hash_combine(int64_t acc, int64_t h) {
    /* Mix in one field hash: FNV-style multiply/xor, then a splitmix64 finalizer
     * for good dispersion so field order matters and small values spread out. */
    uint64_t x = (uint64_t)acc * 1099511628211ULL;
    x ^= (uint64_t)h;
    x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ULL;
    x = (x ^ (x >> 27)) * 0x94d049bb133111ebULL;
    return (int64_t)(x ^ (x >> 31));
}
/* Lexicographic byte comparison of two strings, returning -1 / 0 / +1. */
int64_t zap_str_cmp(void *ap, void *bp) {
    const ZapStr *a = (const ZapStr *)ap;
    const ZapStr *b = (const ZapStr *)bp;
    int64_t n = a->len < b->len ? a->len : b->len;
    int c = memcmp(a->data, b->data, (size_t)n);
    if (c < 0) {
        return -1;
    }
    if (c > 0) {
        return 1;
    }
    if (a->len < b->len) {
        return -1;
    }
    if (a->len > b->len) {
        return 1;
    }
    return 0;
}

static void zap_dict_resize(ZapDict *d, int keykind) {
    int64_t ncap = d->cap < 8 ? 8 : d->cap * 2;
    ZapEntry *ne = (ZapEntry *)zap_ptrbuf((int64_t)ncap * (int64_t)sizeof(ZapEntry));
    for (int64_t i = 0; i < ncap; i++) {
        ne[i].state = SLOT_EMPTY;
    }
    for (int64_t i = 0; i < d->cap; i++) {
        if (d->entries[i].state != SLOT_FULL) {
            continue;
        }
        uint64_t h = zap_key_hash(d->entries[i].key, keykind, d->hashfn);
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
    uint64_t h = zap_key_hash(key, keykind, d->hashfn);
    int64_t j = (int64_t)(h & (uint64_t)(d->cap - 1));
    int64_t tomb = -1;
    while (d->entries[j].state != SLOT_EMPTY) {
        if (d->entries[j].state == SLOT_FULL &&
            zap_key_eq(d->entries[j].key, key, keykind, d->eqfn)) {
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
    uint64_t h = zap_key_hash(key, keykind, d->hashfn);
    int64_t j = (int64_t)(h & (uint64_t)(d->cap - 1));
    while (d->entries[j].state != SLOT_EMPTY) {
        if (d->entries[j].state == SLOT_FULL &&
            zap_key_eq(d->entries[j].key, key, keykind, d->eqfn)) {
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
    ((ZapDict *)d)->val_ptr = 1;
    zap_dict_set(d, k, KEY_I64, (int64_t)(intptr_t)v);
}
void zap_dict_set_str_i64(void *d, void *k, int64_t v) {
    ((ZapDict *)d)->key_ptr = 1;
    zap_dict_set(d, (int64_t)(intptr_t)k, KEY_STR, v);
}
void zap_dict_set_str_ptr(void *d, void *k, void *v) {
    ((ZapDict *)d)->key_ptr = 1;
    ((ZapDict *)d)->val_ptr = 1;
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

/* Struct-keyed dict (operator overloading, D-0047): the key is a struct pointer,
 * hashed/compared through the type's `__hash__`/`__eq__`, passed in as `hashfn`
 * and `eqfn`. They are stored on the dict so resize/probe can reach them. */
void zap_dict_set_fn_i64(void *dp, void *k, int64_t v, void *hashfn, void *eqfn) {
    ZapDict *d = (ZapDict *)dp;
    d->key_ptr = 1;
    d->hashfn = hashfn;
    d->eqfn = eqfn;
    zap_dict_set(dp, (int64_t)(intptr_t)k, KEY_FN, v);
}
void zap_dict_set_fn_ptr(void *dp, void *k, void *v, void *hashfn, void *eqfn) {
    ZapDict *d = (ZapDict *)dp;
    d->key_ptr = 1;
    d->val_ptr = 1;
    d->hashfn = hashfn;
    d->eqfn = eqfn;
    zap_dict_set(dp, (int64_t)(intptr_t)k, KEY_FN, (int64_t)(intptr_t)v);
}
int64_t zap_dict_get_fn_i64(void *dp, void *k, void *hashfn, void *eqfn) {
    ZapDict *d = (ZapDict *)dp;
    d->hashfn = hashfn;
    d->eqfn = eqfn;
    return zap_dict_get(dp, (int64_t)(intptr_t)k, KEY_FN);
}
void *zap_dict_get_fn_ptr(void *dp, void *k, void *hashfn, void *eqfn) {
    ZapDict *d = (ZapDict *)dp;
    d->hashfn = hashfn;
    d->eqfn = eqfn;
    return (void *)(intptr_t)zap_dict_get(dp, (int64_t)(intptr_t)k, KEY_FN);
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
    void *hashfn; /* struct elements only (KEY_FN): the type's __hash__/__eq__ */
    void *eqfn;
    uint8_t elem_ptr; /* elements are GC pointers (str/struct) */
} ZapSet;

void *zap_set_new(void) {
    ZapSet *s = (ZapSet *)zap_obj(&zap_desc_set);
    s->len = 0;
    s->cap = 0;
    s->entries = NULL;
    s->hashfn = NULL;
    s->eqfn = NULL;
    return s;
}

static void zap_set_resize(ZapSet *s, int keykind) {
    int64_t ncap = s->cap < 8 ? 8 : s->cap * 2;
    ZapSetEntry *ne = (ZapSetEntry *)zap_ptrbuf((int64_t)ncap * (int64_t)sizeof(ZapSetEntry));
    for (int64_t i = 0; i < ncap; i++) {
        ne[i].state = SLOT_EMPTY;
    }
    for (int64_t i = 0; i < s->cap; i++) {
        if (s->entries[i].state != SLOT_FULL) {
            continue;
        }
        uint64_t h = zap_key_hash(s->entries[i].key, keykind, s->hashfn);
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
    uint64_t h = zap_key_hash(key, keykind, s->hashfn);
    int64_t j = (int64_t)(h & (uint64_t)(s->cap - 1));
    while (s->entries[j].state != SLOT_EMPTY) {
        if (s->entries[j].state == SLOT_FULL &&
            zap_key_eq(s->entries[j].key, key, keykind, s->eqfn)) {
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
    uint64_t h = zap_key_hash(key, keykind, s->hashfn);
    int64_t j = (int64_t)(h & (uint64_t)(s->cap - 1));
    while (s->entries[j].state != SLOT_EMPTY) {
        if (s->entries[j].state == SLOT_FULL &&
            zap_key_eq(s->entries[j].key, key, keykind, s->eqfn)) {
            return 1;
        }
        j = (j + 1) & (s->cap - 1);
    }
    return 0;
}

int64_t zap_set_len(void *s) { return ((ZapSet *)s)->len; }

void zap_set_add_i64(void *s, int64_t v) { zap_set_add(s, v, KEY_I64); }
void zap_set_add_str(void *s, void *v) {
    ((ZapSet *)s)->elem_ptr = 1;
    zap_set_add(s, (int64_t)(intptr_t)v, KEY_STR);
}
/* Struct-keyed set (operator overloading, D-0047). */
void zap_set_add_fn(void *sp, void *v, void *hashfn, void *eqfn) {
    ZapSet *s = (ZapSet *)sp;
    s->elem_ptr = 1;
    s->hashfn = hashfn;
    s->eqfn = eqfn;
    zap_set_add(sp, (int64_t)(intptr_t)v, KEY_FN);
}
int64_t zap_set_contains_fn(void *sp, void *v, void *hashfn, void *eqfn) {
    ZapSet *s = (ZapSet *)sp;
    s->hashfn = hashfn;
    s->eqfn = eqfn;
    return zap_set_contains(sp, (int64_t)(intptr_t)v, KEY_FN);
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
 * heap[T]: a binary min-heap priority queue (ticket #76, D-0057).
 *
 * A complete binary heap over a growable GC array of 8-byte element words
 * (`int64` values, or `ZapStr` pointer bits). Min-heap by default; the `dir`
 * flag flips the comparator for a max-heap. Elements are *ordered* (not hashed):
 * `int64` words compare numerically, pointer words as `ZapStr` bytes
 * (`zap_str_cmp`). `push` sifts up, `pop` removes the root and sifts down, and
 * `peek` reads the root — all O(log n). Iteration (`zap_heap_items`) copies the
 * backing array and heap-sorts the copy, yielding priority (root-first) order
 * without disturbing the live heap.
 *
 * Deferred (clean follow-ups): user-struct elements ordered through a comparator
 * / `__cmp__` function pointer, float elements, and `cmp=` taking an arbitrary
 * comparator function (the surface is a direction selector for now). */
enum { HEAP_ELEM_I64 = 0, HEAP_ELEM_STR = 1 };
enum { HEAP_MIN = 0, HEAP_MAX = 1 };

typedef struct {
    int64_t len;
    int64_t cap;
    int64_t *data; /* `cap` element words */
    int64_t kind;  /* HEAP_ELEM_I64 | HEAP_ELEM_STR */
    int64_t dir;   /* HEAP_MIN | HEAP_MAX */
} ZapHeap;

/* Three-way compare of two element words under the heap's ordering. Returns < 0
 * when `a` has higher priority than `b` (should sit closer to the root): for a
 * min-heap that is the smaller element, for a max-heap the larger. */
static int64_t zap_heap_cmp(const ZapHeap *h, int64_t a, int64_t b) {
    int64_t c;
    if (h->kind == HEAP_ELEM_STR) {
        c = zap_str_cmp((void *)(intptr_t)a, (void *)(intptr_t)b);
    } else {
        c = (a < b) ? -1 : (a > b ? 1 : 0);
    }
    return h->dir == HEAP_MAX ? -c : c;
}

static void zap_heap_grow(ZapHeap *h) {
    int64_t cap = h->cap < 4 ? 4 : h->cap * 2;
    int64_t *data = (int64_t *)zap_ptrbuf((int64_t)cap * (int64_t)sizeof(int64_t));
    if (h->len > 0) {
        memcpy(data, h->data, (size_t)h->len * sizeof(int64_t));
    }
    h->data = data;
    h->cap = cap;
}

static void zap_heap_swap(ZapHeap *h, int64_t i, int64_t j) {
    int64_t t = h->data[i];
    h->data[i] = h->data[j];
    h->data[j] = t;
}

static void zap_heap_sift_up(ZapHeap *h, int64_t i) {
    while (i > 0) {
        int64_t parent = (i - 1) / 2;
        if (zap_heap_cmp(h, h->data[i], h->data[parent]) < 0) {
            zap_heap_swap(h, i, parent);
            i = parent;
        } else {
            break;
        }
    }
}

static void zap_heap_sift_down(ZapHeap *h, int64_t i) {
    for (;;) {
        int64_t l = 2 * i + 1;
        int64_t r = 2 * i + 2;
        int64_t top = i;
        if (l < h->len && zap_heap_cmp(h, h->data[l], h->data[top]) < 0) {
            top = l;
        }
        if (r < h->len && zap_heap_cmp(h, h->data[r], h->data[top]) < 0) {
            top = r;
        }
        if (top == i) {
            break;
        }
        zap_heap_swap(h, i, top);
        i = top;
    }
}

static void zap_heap_push_word(ZapHeap *h, int64_t word) {
    if (h->len == h->cap) {
        zap_heap_grow(h);
    }
    h->data[h->len] = word;
    zap_heap_sift_up(h, h->len);
    h->len++;
}

/* Remove and return the root word. The caller guarantees the heap is non-empty. */
static int64_t zap_heap_pop_word(ZapHeap *h) {
    int64_t top = h->data[0];
    h->len--;
    if (h->len > 0) {
        h->data[0] = h->data[h->len];
        zap_heap_sift_down(h, 0);
    }
    return top;
}

/* pop/peek on an empty heap aborts (no exception runtime path yet, like an
 * out-of-range list index). */
static void zap_heap_require(const ZapHeap *h, const char *op) {
    if (h->len == 0) {
        fprintf(stderr, "zap: heap %s on an empty heap\n", op);
        abort();
    }
}

void *zap_heap_new(int64_t kind, int64_t dir) {
    ZapHeap *h = (ZapHeap *)zap_obj(&zap_desc_heap);
    h->len = 0;
    h->cap = 0;
    h->data = NULL;
    h->kind = kind;
    h->dir = dir;
    return h;
}

int64_t zap_heap_len(void *h) { return ((ZapHeap *)h)->len; }

void zap_heap_push_i64(void *h, int64_t v) { zap_heap_push_word((ZapHeap *)h, v); }
void zap_heap_push_ptr(void *h, void *v) {
    zap_heap_push_word((ZapHeap *)h, (int64_t)(intptr_t)v);
}

int64_t zap_heap_pop_i64(void *hp) {
    ZapHeap *h = (ZapHeap *)hp;
    zap_heap_require(h, "pop");
    return zap_heap_pop_word(h);
}
void *zap_heap_pop_ptr(void *hp) {
    ZapHeap *h = (ZapHeap *)hp;
    zap_heap_require(h, "pop");
    return (void *)(intptr_t)zap_heap_pop_word(h);
}

int64_t zap_heap_peek_i64(void *hp) {
    ZapHeap *h = (ZapHeap *)hp;
    zap_heap_require(h, "peek");
    return h->data[0];
}
void *zap_heap_peek_ptr(void *hp) {
    ZapHeap *h = (ZapHeap *)hp;
    zap_heap_require(h, "peek");
    return (void *)(intptr_t)h->data[0];
}

/* Snapshot the elements into a fresh list in priority (root-first) order,
 * leaving the live heap untouched: copy the backing array into a scratch heap
 * and drain it. */
void *zap_heap_items(void *hp) {
    ZapHeap *h = (ZapHeap *)hp;
    ZapList *out = (ZapList *)zap_list_new();
    int64_t n = h->len;
    if (n == 0) {
        return out;
    }
    ZapHeap tmp = *h;
    tmp.cap = n;
    tmp.len = n;
    tmp.data = (int64_t *)zap_ptrbuf((int64_t)n * (int64_t)sizeof(int64_t));
    memcpy(tmp.data, h->data, (size_t)n * sizeof(int64_t));
    while (tmp.len > 0) {
        zap_list_push(out, zap_heap_pop_word(&tmp));
    }
    return out;
}

/* ---------------------------------------------------------------------------
 * Numeric arrays (ticket #24, design/0002-arrays.md, design/RUNTIME-ABI.md
 * §"Collection layout": `array[T]`).
 *
 * A dense, contiguous, fixed-dtype buffer. 1-D over the dtypes `f64` and `i64`
 * (8-byte element words) and `uint8`/`bool` (1-byte words; ticket #49, image
 * data and masks). The object carries its dtype and shape so N-D (`ndim > 1`)
 * and strided views can layer on later without an ABI break; this increment
 * always sets `ndim = 1`.
 *
 * The element buffer is pointer-free, so it is allocated with GC_MALLOC_ATOMIC
 * (the collector need not scan it); the shape (`dims`) is likewise atomic. The
 * element-wise kernels (`a + b`, `a * 2.0`, ..., and mixed-dtype promotion) are
 * *not* here: codegen lowers them to explicit LLVM vector operations (SIMD)
 * over `zap_array_data` buffers, which is the point of tickets #24/#49. The
 * runtime only provides construction, element access, length, and scalar
 * reductions. `uint8`/`bool` elements cross the ABI as `int64_t` words (get
 * zero-extends, set truncates to a byte / normalizes to 0|1).
 *
 * Deferred (clean follow-ups): N-D shape + row-major indexing, strides/views
 * (D-0030), broadcasting beyond a scalar, and shape-mismatch runtime checks on
 * element-wise ops. */
enum { DTYPE_F64 = 0, DTYPE_I64 = 1, DTYPE_U8 = 2, DTYPE_BOOL = 3 };

typedef struct {
    int64_t dtype;  /* DTYPE_F64 | DTYPE_I64 | DTYPE_U8 | DTYPE_BOOL */
    int64_t ndim;   /* always 1 this increment */
    int64_t *dims;  /* `ndim` dimensions; dims[0] is the length */
    void *data;     /* contiguous row-major buffer, len * elem-size bytes */
} ZapArray;

/* Bytes per stored element: f64/i64 are 8-byte words, uint8/bool one byte. */
static size_t zap_array_elem_size(int64_t dtype) {
    return (dtype == DTYPE_U8 || dtype == DTYPE_BOOL) ? 1 : 8;
}

void *zap_array_new(int64_t dtype, int64_t len) {
    ZapArray *a = (ZapArray *)zap_obj(&zap_desc_array);
    int64_t *dims = (int64_t *)zap_buf(sizeof(int64_t));
    dims[0] = len;
    /* All current dtypes are pointer-free; the buffer never holds pointers. */
    size_t esz = zap_array_elem_size(dtype);
    void *data = zap_buf((size_t)(len > 0 ? len : 0) * esz);
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

/* uint8 / bool (ticket #49): 1-byte storage, i64 ABI. `get` zero-extends the
 * byte; `set` truncates to a byte (uint8, wraps mod 256) or normalizes to 0|1
 * (bool). `sum` accumulates in int64 so it never overflows the narrow dtype. */
int64_t zap_array_get_u8(void *ap, int64_t i) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    return (int64_t)((const uint8_t *)a->data)[i];
}
void zap_array_set_u8(void *ap, int64_t i, int64_t v) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    ((uint8_t *)a->data)[i] = (uint8_t)v;
}
int64_t zap_array_sum_u8(void *ap) {
    ZapArray *a = (ZapArray *)ap;
    const uint8_t *d = (const uint8_t *)a->data;
    int64_t s = 0;
    for (int64_t i = 0; i < a->dims[0]; i++) {
        s += (int64_t)d[i];
    }
    return s;
}

int64_t zap_array_get_bool(void *ap, int64_t i) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    return ((const uint8_t *)a->data)[i] ? 1 : 0;
}
void zap_array_set_bool(void *ap, int64_t i, int64_t v) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    ((uint8_t *)a->data)[i] = v ? 1 : 0;
}
int64_t zap_array_sum_bool(void *ap) {
    ZapArray *a = (ZapArray *)ap;
    const uint8_t *d = (const uint8_t *)a->data;
    int64_t s = 0;
    for (int64_t i = 0; i < a->dims[0]; i++) {
        s += d[i] ? 1 : 0;
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

/* Decimal string for a double (used by power-assert diagnostics, D-0054).
 * `%g` keeps the rendering compact and round-trippable for typical values. */
void *zap_f64_to_str(double v) {
    char tmp[32];
    int n = snprintf(tmp, sizeof(tmp), "%g", v);
    return zap_str_new(tmp, (int64_t)n);
}

/* `"True"` / `"False"` for a boolean (power-assert diagnostics, D-0054). */
void *zap_bool_to_str(int64_t v) {
    return v ? zap_str_new("True", 4) : zap_str_new("False", 5);
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
            (ZapReflectEntry *)zap_ptrbuf((int64_t)ncap * (int64_t)sizeof(ZapReflectEntry));
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

    ZapMatch *m = (ZapMatch *)zap_obj(&zap_desc_matchobj);
    m->ngroups = ngroups;
    m->groups = (ZapStr **)zap_ptrbuf((int64_t)ngroups * (int64_t)sizeof(ZapStr *));
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
        m->names = (ZapStr **)zap_ptrbuf((int64_t)namecount * (int64_t)sizeof(ZapStr *));
        m->name_num = (int64_t *)zap_buf((int64_t)namecount * (int64_t)sizeof(int64_t));
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
    PCRE2_UCHAR *buf = (PCRE2_UCHAR *)zap_buf((size_t)outlen);
    int rc = pcre2_substitute(re, (PCRE2_SPTR)subject->data, (PCRE2_SIZE)subject->len, 0, opts,
                              NULL, NULL, (PCRE2_SPTR)replacement->data,
                              (PCRE2_SIZE)replacement->len, buf, &outlen);
    if (rc == PCRE2_ERROR_NOMEMORY) {
        buf = (PCRE2_UCHAR *)zap_buf((size_t)outlen);
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
    ZapPy *h = (ZapPy *)zap_obj(&zap_desc_pyobj);
    h->obj = o;
    GC_register_finalizer(h, zap_py_finalize, NULL, NULL, NULL);
    return h;
}

/* NUL-terminated copy of a ZapStr (for the C-string CPython APIs). */
static char *zap_py_cstr(const ZapStr *s) {
    char *buf = (char *)zap_buf((size_t)s->len + 1);
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
 * Goroutines + channels on an M:N work-stealing scheduler
 * (ticket #47, D-0008/D-0013/D-0031/D-0033/D-0034, design/0005-concurrency.md).
 *
 * This replaces the 1:1 pthread-backed goroutine runtime shipped in #25/#46
 * with a user-space scheduler. Language-visible semantics are UNCHANGED; this
 * is purely a runtime swap, and every `zap_*` entry point below keeps the exact
 * signature #25 compiled against, so lowering/codegen are untouched (the one
 * additive codegen change is the spawn trampoline's landing pad, which only
 * affects the uncaught-in-goroutine path).
 *
 * The Go M:P:G structure:
 *   - G  a goroutine: a fixed-size green-thread stack + a saved register
 *        context, allocated on the GC heap.
 *   - M  an OS worker thread, GC-registered, that runs Gs.
 *   - P  a processor context owned by one M, holding a local run queue; idle
 *        Ms steal from other Ps and from a global run queue (load balance with
 *        no central lock on the hot path).
 *
 * Green-thread stacks and the Boehm interaction (design/0005 "Green-thread
 * stacks"): each G's stack is a `GC_MALLOC` object (non-atomic, pointer-bearing)
 * and its saved `ucontext_t` lives inside the GC-allocated `G`. So a *suspended*
 * G's stack and callee-saved registers are retained and conservatively scanned
 * *as heap* with no stack maps or safepoints — the key trick. Every live G is
 * additionally kept in the statically-rooted `g_all[]` registry, so a parked G
 * (otherwise reachable only from a Waiter on its own stack) is never collected.
 * A *running* G executes on its green stack on some M; so that the collector
 * scans that stack as the M thread's stack (and does not scan a bogus
 * cross-stack range), each context switch updates the M thread's GC stack
 * bottom via `GC_set_stackbottom` (bdwgc's documented coroutine support).
 *
 * Context switch (design/0005 "Interaction with exception unwinding"):
 * ucontext `swapcontext` saves/restores callee-saved regs + SP + IP. ARCH
 * ASSUMPTION: ucontext is available and correct on the Linux x86_64 dev
 * container (LLVM 18 / glibc). The spawn trampoline is each G's base frame; an
 * uncaught exception is caught there by a landing pad (codegen), so unwinding
 * never crosses into the makecontext bootstrap. A G suspended mid-unwind
 * resumes its unwind correctly because CFI is driven per frame on its own
 * stack.
 *
 * Blocking (design/0005 "Runtime entry points"): channels park the G and free
 * the M on a scheduler-keyed wait queue rather than blocking the OS thread on a
 * condvar. A non-goroutine caller (the main thread) instead blocks on a
 * per-waiter condvar, which parks that OS thread without consuming a worker M.
 *
 * DEFERRED (documented in the PR / OPEN-QUESTIONS; semantics unchanged): the
 * netpoller / blocking-syscall P-handoff, preemption of long CPU loops (v1 is
 * cooperative — Gs yield only at channel ops), hardware guard pages and
 * growable/segmented stacks (a PROT_NONE guard page inside a GC_MALLOC object
 * would break conservative scanning; the spec ties real guard/growable stacks
 * to the precise-GC era, so v1 uses a fixed stack with a soft overflow check),
 * and the `sync` module / `scope:` / `select` surface still pending from #25.
 * =========================================================================== */

/* Fixed green-thread stack size (design/0005: 64-256 KiB fixed, no growth in
 * v1). Overridable via ZAP_STACK_SIZE (KiB) for experimentation. */
#define ZAP_STACK_DEFAULT (128 * 1024)
#define ZAP_STACK_REDZONE 8192 /* soft overflow margin checked at park points */

typedef enum { G_RUNNABLE, G_RUNNING, G_PARKED, G_DEAD } GState;

typedef struct G {
    ucontext_t ctx;    /* saved callee-saved regs + SP + IP (scanned as heap) */
    void *stack;       /* GC_MALLOC'd green stack buffer (scanned as heap)     */
    size_t stack_size;
    struct GC_stack_base sb; /* cold end (high address) of this G's stack      */
    void *fn;          /* trampoline `void *(*)(void *)` from zap_spawn        */
    void *arg;         /* closure passed to the trampoline                     */
    GState state;
    struct G *link;    /* run-queue / free-list link (a G is in one at a time) */
    pthread_mutex_t *parkmu; /* chan mutex the scheduler unlocks after parking */
    int64_t all_idx;   /* index in g_all[] for O(1) removal on death           */
    struct Scope *scope; /* owning `scope:` (ticket #50), or NULL (root/detached)*/
    int in_live;       /* 1 iff this G is counted in g_live (root-scope join)   */
    void *shadow_head; /* precise GC (#48): saved shadow-stack head when this G
                        * is not running on an M. A running G's live head is in
                        * the per-thread zap_shadow_head, flushed here at a
                        * safepoint; the collector walks this per parked G. */
} G;

typedef struct P {
    pthread_mutex_t mu; /* guards this P's local deque                         */
    G *head;            /* LIFO: push/pop at head (locality), steal at tail    */
    G *tail;
} P;

typedef struct M {
    pthread_t thread;
    ucontext_t sched_ctx;       /* the scheduler loop's context (native stack) */
    struct GC_stack_base native_sb; /* this M's native stack cold end          */
    void *gc_handle;            /* GC thread handle for GC_set_stackbottom      */
    G *current;                 /* G running on this M, or NULL in scheduler    */
    P *p;                       /* processor this M is attached to              */
} M;

/* Per-waiter node on a channel's sender/receiver queue. A goroutine waiter
 * (`g != NULL`) is re-enqueued on the scheduler when woken; a native-thread
 * waiter blocks on its own condvar. The waker fills `val`/`ok`/`closed` under
 * the channel lock *before* waking, so the woken waiter reads its outcome
 * without re-locking. */
typedef struct Waiter {
    struct Waiter *next;
    G *g;                 /* NULL => native-thread (e.g. main) waiter          */
    pthread_cond_t cv;    /* native waiter only                                */
    volatile int done;    /* native waiter: set by the waker                   */
    uint64_t val;         /* send: value offered; recv: value delivered        */
    int ok;               /* recv: 1 iff a real value was delivered            */
    int closed;           /* set iff the channel closed while this waiter slept */
} Waiter;

/* ---- scheduler globals ---------------------------------------------------- */
static __thread M *cur_m = NULL; /* the M running this thread; NULL on main    */

static M *g_ms = NULL;
static P *g_ps = NULL;
static int g_nm = 0;             /* number of Ms / Ps                          */

static pthread_mutex_t g_global_mu = PTHREAD_MUTEX_INITIALIZER; /* global runq */
static G *g_global_head = NULL;
static G *g_global_tail = NULL;

static pthread_mutex_t g_idle_mu = PTHREAD_MUTEX_INITIALIZER; /* idle-M parking */
static pthread_cond_t g_idle_cv = PTHREAD_COND_INITIALIZER;
static volatile int g_shutdown = 0;

static pthread_mutex_t g_live_mu = PTHREAD_MUTEX_INITIALIZER; /* live-G count   */
static pthread_cond_t g_live_cv = PTHREAD_COND_INITIALIZER;
static int64_t g_live = 0;       /* runnable+running+parked Gs (root-scope join) */

/* Statically-rooted registry of every live G, so parked Gs stay reachable. */
static pthread_mutex_t g_all_mu = PTHREAD_MUTEX_INITIALIZER;
static G **g_all = NULL;
static int64_t g_all_len = 0;
static int64_t g_all_cap = 0;

/* Size-classed free list of finished Gs (one class: the fixed stack size). */
static pthread_mutex_t g_free_mu = PTHREAD_MUTEX_INITIALIZER;
static G *g_free_head = NULL;

static size_t zap_stack_size(void) {
    static size_t sz = 0;
    if (sz == 0) {
        sz = ZAP_STACK_DEFAULT;
        const char *e = getenv("ZAP_STACK_SIZE");
        if (e) {
            long kib = atol(e);
            if (kib >= 32 && kib <= 8192) {
                sz = (size_t)kib * 1024;
            }
        }
    }
    return sz;
}

/* ---- GC stack-bottom tracking across switches ----------------------------- */
/* Point the current M thread's conservatively-scanned stack at `sb`. Wrapped in
 * the GC alloc lock (as bdwgc advises) so a concurrent collection never reads a
 * half-updated bottom. The brief window between this and the swapcontext that
 * moves SP is the same one bdwgc fiber runtimes accept; green stacks are also
 * scanned as heap, so no root is ever missed. */
struct sb_arg {
    void *handle;
    struct GC_stack_base *sb;
};
static void *set_bottom_locked(void *p) {
    struct sb_arg *a = (struct sb_arg *)p;
    GC_set_stackbottom(a->handle, a->sb);
    return NULL;
}
static void set_bottom(M *m, struct GC_stack_base *sb) {
    struct sb_arg a; /* per-call, on the caller's stack: no cross-M race */
    a.handle = m->gc_handle;
    a.sb = sb;
    GC_call_with_alloc_lock(set_bottom_locked, &a);
}

/* ---- run queues ----------------------------------------------------------- */
static void global_push(G *g) {
    pthread_mutex_lock(&g_global_mu);
    g->link = NULL;
    if (g_global_tail) {
        g_global_tail->link = g;
    } else {
        g_global_head = g;
    }
    g_global_tail = g;
    pthread_mutex_unlock(&g_global_mu);
}
static G *global_pop(void) {
    pthread_mutex_lock(&g_global_mu);
    G *g = g_global_head;
    if (g) {
        g_global_head = g->link;
        if (!g_global_head) {
            g_global_tail = NULL;
        }
        g->link = NULL;
    }
    pthread_mutex_unlock(&g_global_mu);
    return g;
}
static void p_push(P *p, G *g) {
    pthread_mutex_lock(&p->mu);
    g->link = p->head;
    p->head = g;
    if (!p->tail) {
        p->tail = g;
    }
    pthread_mutex_unlock(&p->mu);
}
static G *p_pop(P *p) { /* LIFO from head (owner) */
    pthread_mutex_lock(&p->mu);
    G *g = p->head;
    if (g) {
        p->head = g->link;
        if (!p->head) {
            p->tail = NULL;
        }
        g->link = NULL;
    }
    pthread_mutex_unlock(&p->mu);
    return g;
}
static G *p_steal(P *p) { /* FIFO from tail (thief) */
    pthread_mutex_lock(&p->mu);
    G *g = p->head, *prev = NULL;
    if (g) {
        while (g->link) {
            prev = g;
            g = g->link;
        }
        if (prev) {
            prev->link = NULL;
            p->tail = prev;
        } else {
            p->head = p->tail = NULL;
        }
        g->link = NULL;
    }
    pthread_mutex_unlock(&p->mu);
    return g;
}

/* Make `g` runnable: push to the current M's P (locality) or the global queue,
 * then wake one idle M. */
static void sched_enqueue(G *g) {
    g->state = G_RUNNABLE;
    if (cur_m && cur_m->p) {
        p_push(cur_m->p, g);
    } else {
        global_push(g);
    }
    pthread_mutex_lock(&g_idle_mu);
    pthread_cond_signal(&g_idle_cv);
    pthread_mutex_unlock(&g_idle_mu);
}

static G *sched_find_runnable(M *m) {
    for (;;) {
        G *g = p_pop(m->p);
        if (g) {
            return g;
        }
        g = global_pop();
        if (g) {
            return g;
        }
        for (int i = 0; i < g_nm; i++) {
            if (&g_ps[i] != m->p) {
                g = p_steal(&g_ps[i]);
                if (g) {
                    return g;
                }
            }
        }
        /* Nothing to run: park this M until work arrives or we shut down. A
         * short timed wait re-polls, closing any enqueue/signal lost-wakeup. */
        pthread_mutex_lock(&g_idle_mu);
        if (g_shutdown) {
            pthread_mutex_unlock(&g_idle_mu);
            return NULL;
        }
        struct timespec ts;
        clock_gettime(CLOCK_REALTIME, &ts);
        ts.tv_nsec += 1000000; /* 1 ms */
        if (ts.tv_nsec >= 1000000000) {
            ts.tv_sec++;
            ts.tv_nsec -= 1000000000;
        }
        pthread_cond_timedwait(&g_idle_cv, &g_idle_mu, &ts);
        pthread_mutex_unlock(&g_idle_mu);
    }
}

/* Scheduler-side switch into `g`. Brackets the swapcontext with the GC
 * stack-bottom update so the collector always scans the correct stack for this
 * M. On return the G has either parked (unlock the channel mutex it asked us to
 * release, completing the park/wake handoff) or finished (reclaim it). */
static void gfree(G *g);
static void sched_gdone(void);
static void scope_gdone(struct Scope *sc);
static void sched_enqueue(G *g);
static void switch_to_g(M *m, G *g) {
    m->current = g;
    g->state = G_RUNNING;
    set_bottom(m, &g->sb);
#ifdef ZAP_GC_IMMIX
    /* Restore this coroutine's shadow-stack head into the per-thread cell the
     * compiled code mutates; save it back when the G switches out (parks /
     * yields / dies). A running G's head thus lives in zap_shadow_head and is
     * flushed to g->shadow_head at a safepoint or here. */
    zap_shadow_head = g->shadow_head;
#endif
    swapcontext(&m->sched_ctx, &g->ctx);
#ifdef ZAP_GC_IMMIX
    g->shadow_head = zap_shadow_head;
#endif
    set_bottom(m, &m->native_sb);
    m->current = NULL;
    if (g->state == G_PARKED) {
        if (g->parkmu) {
            pthread_mutex_t *mu = g->parkmu;
            g->parkmu = NULL;
            pthread_mutex_unlock(mu);
        }
    } else if (g->state == G_DEAD) {
        /* Capture ownership before gfree clears it, then signal the right join
         * barrier: a scoped G releases its `scope:` (ticket #50); a root G
         * releases the program-exit join; a detached G is joined by nobody. */
        struct Scope *sc = g->scope;
        int in_live = g->in_live;
        gfree(g);
        if (sc) {
            scope_gdone(sc);
        } else if (in_live) {
            sched_gdone();
        }
    } else if (g->state == G_RUNNABLE) {
        /* The G yielded (`gosched`, e.g. at `checkpoint()`): requeue it. */
        sched_enqueue(g);
    }
}

static void *g_entry_fn(G *g); /* fwd */

/* The makecontext bootstrap: runs on the green stack. Re-reads `cur_m` after
 * the call returns because a G may be resumed on a different M than it started
 * on. Never returns — switches back to whatever M's scheduler is running it. */
static void g_bootstrap(void) {
    M *m = cur_m;
    G *g = m->current;
    g_entry_fn(g);
    m = cur_m; /* may have migrated across parks */
    g->state = G_DEAD;
    swapcontext(&g->ctx, &m->sched_ctx);
    /* unreachable */
}

static void *g_entry_fn(G *g) {
    void *(*fn)(void *) = (void *(*)(void *))g->fn;
    return fn(g->arg);
}

/* ---- worker thread -------------------------------------------------------- */
static void *worker_main(void *arg) {
    M *m = (M *)arg;
    cur_m = m;
    /* This thread was created by GC_pthread_create, so it is registered; record
     * its GC handle + native stack bottom for the switch bracketing. */
    struct GC_stack_base sb;
    m->gc_handle = GC_get_my_stackbottom(&sb);
    m->native_sb = sb;
    /* Precise GC (#48): this worker runs compiled Zap on Gs, so register it as a
     * mutator. It is BLOCKED (GC-safe) while in the scheduler loop and ACTIVE
     * while a G runs on it (the G polls safepoints); zap_safepoint parks it when
     * a collection is due. */
    zap_mutator_register();
    for (;;) {
        zap_enter_blocking(); /* scheduler loop: GC-safe, no un-flushed roots */
        G *g = sched_find_runnable(m);
        if (!g) {
            return NULL; /* shutdown */
        }
        zap_leave_blocking(); /* about to run Zap; wait out any in-flight GC */
        switch_to_g(m, g);
    }
}

/* ---- G lifecycle ---------------------------------------------------------- */
static void all_add(G *g) {
    pthread_mutex_lock(&g_all_mu);
    if (g_all_len == g_all_cap) {
        int64_t ncap = g_all_cap ? g_all_cap * 2 : 16;
        G **n = (G **)ZAP_INFRA_ALLOC((size_t)ncap * sizeof(G *));
        if (g_all) {
            memcpy(n, g_all, (size_t)g_all_len * sizeof(G *));
        }
        g_all = n;
        g_all_cap = ncap;
    }
    g->all_idx = g_all_len;
    g_all[g_all_len++] = g;
    pthread_mutex_unlock(&g_all_mu);
}
static void all_remove(G *g) {
    pthread_mutex_lock(&g_all_mu);
    int64_t i = g->all_idx;
    if (i >= 0 && i < g_all_len && g_all[i] == g) {
        G *last = g_all[--g_all_len];
        g_all[i] = last;
        last->all_idx = i;
        g_all[g_all_len] = NULL;
    }
    pthread_mutex_unlock(&g_all_mu);
}

static G *galloc(void *fn, void *arg) {
    size_t ss = zap_stack_size();
    G *g = NULL;
    pthread_mutex_lock(&g_free_mu);
    if (g_free_head) {
        g = g_free_head;
        g_free_head = g->link;
    }
    pthread_mutex_unlock(&g_free_mu);
    if (!g) {
        g = (G *)zap_obj(&zap_desc_gobj);
        g->stack = zap_ptrbuf((int64_t)ss); /* Boehm: scanned as heap. Immix: atomic buffer kept alive via the G.stack precise field; roots come from the shadow chain, not stack scanning. */
        g->stack_size = ss;
    }
    g->fn = fn;
    g->arg = arg;
    g->state = G_RUNNABLE;
    g->link = NULL;
    g->parkmu = NULL;
    g->scope = NULL;
    g->in_live = 0;
    g->shadow_head = NULL; /* precise GC (#48): a fresh G has an empty chain */
    /* Cold end of a down-growing stack is its high address. */
    g->sb.mem_base = (char *)g->stack + g->stack_size;
    getcontext(&g->ctx);
    g->ctx.uc_stack.ss_sp = g->stack;
    g->ctx.uc_stack.ss_size = g->stack_size;
    g->ctx.uc_link = NULL; /* bootstrap never returns */
    makecontext(&g->ctx, g_bootstrap, 0);
    all_add(g);
    return g;
}

static void gfree(G *g) {
    all_remove(g);
    g->fn = NULL;
    g->arg = NULL;
    g->scope = NULL; /* drop the scope reference so a reused G starts clean */
    g->in_live = 0;
    /* Keep the stack buffer for reuse (size-classed free list, one class). */
    pthread_mutex_lock(&g_free_mu);
    g->link = g_free_head;
    g_free_head = g;
    pthread_mutex_unlock(&g_free_mu);
}

static void sched_gdone(void) {
    pthread_mutex_lock(&g_live_mu);
    if (--g_live == 0) {
        pthread_cond_broadcast(&g_live_cv);
    }
    pthread_mutex_unlock(&g_live_mu);
}

/* ---- scheduler init (lazy, on first spawn) -------------------------------- */
static pthread_once_t g_sched_once = PTHREAD_ONCE_INIT;

static int zap_maxprocs(void) {
    const char *e = getenv("ZAP_MAXPROCS");
    if (e) {
        long n = atol(e);
        if (n >= 1 && n <= 1024) {
            return (int)n;
        }
    }
    long hw = sysconf(_SC_NPROCESSORS_ONLN);
    if (hw < 1) {
        hw = 1;
    }
    if (hw > 256) {
        hw = 256;
    }
    return (int)hw;
}

static void zap_sched_init_once(void) {
    g_nm = zap_maxprocs();
    g_ps = (P *)ZAP_INFRA_ALLOC((size_t)g_nm * sizeof(P));
    g_ms = (M *)ZAP_INFRA_ALLOC((size_t)g_nm * sizeof(M));
    for (int i = 0; i < g_nm; i++) {
        pthread_mutex_init(&g_ps[i].mu, NULL);
        g_ps[i].head = g_ps[i].tail = NULL;
        memset(&g_ms[i], 0, sizeof(M));
        g_ms[i].p = &g_ps[i];
    }
    for (int i = 0; i < g_nm; i++) {
        int rc = GC_pthread_create(&g_ms[i].thread, NULL, worker_main, &g_ms[i]);
        if (rc != 0) {
            fprintf(stderr, "zap: failed to start scheduler thread (%d)\n", rc);
            abort();
        }
    }
}

/* `zap_sched_init(n_threads)` — the design's named entry point. A value <= 0
 * means "use the default" (ZAP_MAXPROCS / hardware threads). The scheduler is
 * started at most once; an explicit n only takes effect if it is the first call
 * (before any spawn), honoring the env override otherwise. */
void zap_sched_init(int64_t n_threads) {
    if (n_threads > 0 && !g_ms) {
        /* Seed the env-less default so zap_maxprocs picks this up. */
        static char buf[32];
        if (!getenv("ZAP_MAXPROCS")) {
            snprintf(buf, sizeof(buf), "%lld", (long long)n_threads);
            setenv("ZAP_MAXPROCS", buf, 0);
        }
    }
    pthread_once(&g_sched_once, zap_sched_init_once);
}

/* ---- spawn + root-scope join ---------------------------------------------- */
/* `zap_spawn(trampoline, closure)` keeps #25's signature. The trampoline is the
 * codegen-synthesized `void *(*)(void *)` that unpacks the eagerly-evaluated
 * argument closure and calls the target; here it becomes a new G enqueued on the
 * scheduler (G alloc + enqueue, design/0005 "Runtime entry points"). */
void zap_spawn(void *fn_, void *arg) {
    zap_sched_init(0);
    pthread_mutex_lock(&g_live_mu);
    g_live++;
    pthread_mutex_unlock(&g_live_mu);
    G *g = galloc(fn_, arg);
    g->in_live = 1; /* joined by the implicit root scope at program exit */
    sched_enqueue(g);
}

/* `spawn detached <call>` (ticket #50, D-0044): a coroutine owned by nothing —
 * never awaited, and not counted in the program-exit join, so a still-running
 * detached coroutine is terminated when `main` returns (Go goroutine
 * semantics). */
void zap_spawn_detached(void *fn_, void *arg) {
    zap_sched_init(0);
    G *g = galloc(fn_, arg);
    /* in_live stays 0 and scope stays NULL: no join barrier owns it. */
    sched_enqueue(g);
}

/* The implicit root scope's join at program exit (emitted at `main`'s return):
 * block until every spawned G (and its descendants) has finished, then stop the
 * worker Ms. A no-op if nothing ever spawned (scheduler never started). */
void zap_rt_join_all(void) {
    if (!g_ms) {
        return;
    }
    /* Precise GC (#48): the main thread blocks here until every spawned G
     * finishes. Mark it GC-safe so collections triggered by the workers proceed
     * without waiting for the (idle) main thread. */
    zap_enter_blocking();
    pthread_mutex_lock(&g_live_mu);
    while (g_live > 0) {
        pthread_cond_wait(&g_live_cv, &g_live_mu);
    }
    pthread_mutex_unlock(&g_live_mu);
    zap_leave_blocking();
    pthread_mutex_lock(&g_idle_mu);
    g_shutdown = 1;
    pthread_cond_broadcast(&g_idle_cv);
    pthread_mutex_unlock(&g_idle_mu);
    for (int i = 0; i < g_nm; i++) {
        GC_pthread_join(g_ms[i].thread, NULL);
    }
}

/* ---- structured concurrency: scope / detached / ExceptionGroup ------------ *
 * (ticket #50, D-0044/D-0045, design/0005-concurrency.md "Structured
 * concurrency"). A `scope:` block owns every coroutine spawned in it and is the
 * join point at its dedent:
 *   - normal exit  -> `zap_scope_exit`: await all owned coroutines; if any
 *     failed, re-raise — a single exception, or an aggregate `ExceptionGroup`
 *     carrying all of them when several failed concurrently (D-0045).
 *   - failure / early exit -> `zap_scope_exit_cancel`: request cancellation,
 *     await the owned coroutines, and let the parent's in-flight exception win
 *     (its children's are discarded).
 *
 * Cancellation is cooperative (design/0005 "Cancellation is cooperative"): a
 * cancelled scope arms its coroutines, and the cancel takes effect at the next
 * cancellation point reached — a channel send/receive or an explicit
 * `checkpoint()` — where the runtime raises `Cancelled`. `Cancelled` rides the
 * ordinary unwinder (so `finally`/`with` cleanups run) and is *not* recorded as
 * a scope failure. Interrupting a coroutine already parked on a channel (rather
 * than one that next reaches a cancellation point) needs the netpoller-style
 * wakeup that is a documented deferral, as is parking — rather than blocking the
 * worker M — at a join reached from inside a coroutine (a nested `scope:`); both
 * work at the default MAXPROCS and the shapes the goldens exercise. */

typedef struct Scope {
    pthread_mutex_t mu;
    pthread_cond_t cv;      /* broadcast when `live` reaches 0 (join barrier)   */
    int64_t live;          /* owned coroutines not yet finished                */
    int cancelled;         /* cancellation requested (armed cooperatively)     */
    int on_error;          /* 0 = Cancel (default), 1 = Wait (D-0044)          */
    ZapStr **excs;         /* collected failures from owned coroutines (GC)    */
    int64_t nexc;
    int64_t exc_cap;
} Scope;

/* The shared `Cancelled` sentinel (a BaseException-ish marker, design/0005): a
 * single cached ZapStr whose pointer identity marks a cooperative cancel, so a
 * cancelled sibling is told apart from a genuine failure by identity, not text.
 * Held in a GC-scanned static, so it stays live. */
static ZapStr *g_cancelled = NULL;
static pthread_mutex_t g_cancelled_mu = PTHREAD_MUTEX_INITIALIZER;
static ZapStr *cancelled_exc(void) {
    pthread_mutex_lock(&g_cancelled_mu);
    if (!g_cancelled) {
        g_cancelled = (ZapStr *)zap_str_new("Cancelled", 9);
    }
    pthread_mutex_unlock(&g_cancelled_mu);
    return g_cancelled;
}

/* Yield the current coroutine back to the scheduler (a cooperative scheduling
 * point). A no-op on a native thread (e.g. `main`), which does not run on an M.
 * The scheduler requeues a G left in G_RUNNABLE (see `switch_to_g`). */
static void gosched(void) {
    M *m = cur_m;
    if (!m || !m->current) {
        return;
    }
    G *g = m->current;
    char probe;
    if ((char *)&probe - (char *)g->stack < ZAP_STACK_REDZONE) {
        zap_panic(zap_str_new("stack overflow", 14));
        return;
    }
    g->state = G_RUNNABLE;
    swapcontext(&g->ctx, &m->sched_ctx);
}

/* Is the current coroutine's owning scope cancelled? (0 on a native thread or an
 * unowned coroutine.) */
static int g_is_cancelled(void) {
    M *m = cur_m;
    G *g = m ? m->current : NULL;
    if (!g || !g->scope) {
        return 0;
    }
    Scope *sc = g->scope;
    pthread_mutex_lock(&sc->mu);
    int c = sc->cancelled;
    pthread_mutex_unlock(&sc->mu);
    return c;
}

/* A cancellation point: raise `Cancelled` if the current coroutine was armed. */
static void zap_check_cancel(void) {
    if (g_is_cancelled()) {
        zap_panic((void *)cancelled_exc());
    }
}

/* `checkpoint()` (D-0044): an explicit cancellation point for long CPU-bound
 * loops. Observes a pending cancel, then yields so siblings make progress. */
void zap_checkpoint(void) {
    zap_check_cancel();
    gosched();
}

/* An owned coroutine finished: drop it from its scope's live count and wake the
 * join barrier when the last one leaves. */
static void scope_gdone(Scope *sc) {
    pthread_mutex_lock(&sc->mu);
    if (--sc->live == 0) {
        pthread_cond_broadcast(&sc->cv);
    }
    pthread_mutex_unlock(&sc->mu);
}

/* `zap_scope_enter(on_error)` opens a scope (ticket #50). The handle is returned
 * to the generated code, which keeps it live on its (conservatively scanned)
 * frame and hands it to each owned `spawn` and to the matching exit call. */
void *zap_scope_enter(int64_t on_error) {
    Scope *sc = (Scope *)zap_obj(&zap_desc_scope);
    pthread_mutex_init(&sc->mu, NULL);
    pthread_cond_init(&sc->cv, NULL);
    sc->live = 0;
    sc->cancelled = 0;
    sc->on_error = on_error == 1 ? 1 : 0;
    sc->excs = NULL;
    sc->nexc = 0;
    sc->exc_cap = 0;
    return sc;
}

/* `spawn <call>` inside a `scope:` (ticket #50): a coroutine owned by `scope`,
 * awaited at the scope's dedent rather than at program exit. */
void zap_spawn_scoped(void *fn_, void *arg, void *scope_) {
    zap_sched_init(0);
    Scope *sc = (Scope *)scope_;
    pthread_mutex_lock(&sc->mu);
    sc->live++;
    pthread_mutex_unlock(&sc->mu);
    G *g = galloc(fn_, arg);
    g->scope = sc; /* in_live stays 0: the scope, not the root join, owns it */
    sched_enqueue(g);
}

/* Block until every coroutine owned by `sc` has finished. Called from `main`
 * (a native thread) for a top-level scope; a scope reached from inside a
 * coroutine blocks that worker M here (the park-the-G variant is deferred). */
static void scope_join(Scope *sc) {
    /* Precise GC (#48): GC-safe while blocked (see chan_block / zap_rt_join_all).
     * When reached from inside a coroutine this blocks the worker M; marking it
     * BLOCKED lets a concurrent collection proceed. The scope's pending-failure
     * exceptions (sc->excs) stay rooted via the scope object itself. */
    zap_enter_blocking();
    pthread_mutex_lock(&sc->mu);
    while (sc->live > 0) {
        pthread_cond_wait(&sc->cv, &sc->mu);
    }
    pthread_mutex_unlock(&sc->mu);
    zap_leave_blocking();
}

/* Order the collected failures deterministically (by message bytes) so an
 * `ExceptionGroup` built from concurrent failures has a stable rendering. */
static int exc_cmp(const void *a, const void *b) {
    const ZapStr *x = *(const ZapStr *const *)a;
    const ZapStr *y = *(const ZapStr *const *)b;
    int64_t n = x->len < y->len ? x->len : y->len;
    int c = n > 0 ? memcmp(x->data, y->data, (size_t)n) : 0;
    if (c != 0) {
        return c;
    }
    return x->len < y->len ? -1 : (x->len > y->len ? 1 : 0);
}

/* Build the aggregate `ExceptionGroup` message for `n (>= 2)` concurrent
 * failures (D-0045). The typed `ExceptionGroup` object is a follow-up (the
 * exception increment carries the message string only, RUNTIME-ABI); this
 * renders the aggregate as a deterministic multi-line message. */
static void *make_exception_group(ZapStr **excs, int64_t n) {
    ZapStr **sorted = (ZapStr **)zap_ptrbuf((int64_t)n * (int64_t)sizeof(ZapStr *));
    memcpy(sorted, excs, (size_t)n * sizeof(ZapStr *));
    qsort(sorted, (size_t)n, sizeof(ZapStr *), exc_cmp);
    char header[64];
    int hlen = snprintf(header, sizeof(header),
                        "ExceptionGroup (%lld sub-exceptions):", (long long)n);
    int64_t total = hlen;
    for (int64_t i = 0; i < n; i++) {
        total += 5 + (sorted[i] ? sorted[i]->len : 0); /* "\n  - " + message */
    }
    char *buf = (char *)zap_buf((size_t)total);
    int64_t off = 0;
    memcpy(buf + off, header, (size_t)hlen);
    off += hlen;
    for (int64_t i = 0; i < n; i++) {
        memcpy(buf + off, "\n  - ", 5);
        off += 5;
        if (sorted[i] && sorted[i]->len > 0) {
            memcpy(buf + off, sorted[i]->data, (size_t)sorted[i]->len);
            off += sorted[i]->len;
        }
    }
    return zap_str_new(buf, off);
}

/* Normal-exit join (D-0044 rule 2/3): await the owned coroutines, then re-raise
 * their failure — the single exception, or an `ExceptionGroup` when several
 * failed concurrently (D-0045). No failure => returns normally. */
void zap_scope_exit(void *scope_) {
    Scope *sc = (Scope *)scope_;
    scope_join(sc);
    if (sc->nexc == 0) {
        return;
    }
    if (sc->nexc == 1) {
        zap_panic(sc->excs[0]);
    }
    zap_panic(make_exception_group(sc->excs, sc->nexc));
}

/* Early-exit/failure join (D-0044 rule 4): the parent block is leaving with its
 * own in-flight exception, so cancel the owned coroutines, await them, and
 * return — the generated landing pad then re-raises the parent's exception. Any
 * failures the children recorded are discarded (the parent's exception wins). */
void zap_scope_exit_cancel(void *scope_) {
    Scope *sc = (Scope *)scope_;
    pthread_mutex_lock(&sc->mu);
    sc->cancelled = 1;
    pthread_mutex_unlock(&sc->mu);
    scope_join(sc);
}

/* An uncaught exception reached a coroutine's base frame (ticket #50, replacing
 * the blanket `zap_uncaught` the spawn thunk used before). Decide by ownership:
 *   - owned by a `scope:` -> record the failure (unless it is the cooperative
 *     `Cancelled` sentinel, which is expected and dropped) and, under
 *     OnError.Cancel, arm the scope so siblings unwind too; then return so the
 *     coroutine ends — the scope re-raises at its dedent.
 *   - root/detached -> abort the program with the message (exit 1), exactly the
 *     bare-`spawn` behavior from #25 (design/0005 "Uncaught exceptions"). */
void zap_g_uncaught(void *exc) {
    M *m = cur_m;
    G *g = m ? m->current : NULL;
    if (g && g->scope) {
        Scope *sc = g->scope;
        if (exc == (void *)cancelled_exc()) {
            return; /* a cooperatively-cancelled sibling: not a failure */
        }
        pthread_mutex_lock(&sc->mu);
        if (sc->nexc == sc->exc_cap) {
            int64_t ncap = sc->exc_cap ? sc->exc_cap * 2 : 4;
            ZapStr **nx = (ZapStr **)zap_ptrbuf((int64_t)ncap * (int64_t)sizeof(ZapStr *));
            if (sc->excs) {
                memcpy(nx, sc->excs, (size_t)sc->nexc * sizeof(ZapStr *));
            }
            sc->excs = nx;
            sc->exc_cap = ncap;
        }
        sc->excs[sc->nexc++] = (ZapStr *)exc;
        if (sc->on_error == 0) {
            sc->cancelled = 1; /* OnError.Cancel: a failure cancels siblings */
        }
        pthread_mutex_unlock(&sc->mu);
        return;
    }
    zap_uncaught(exc); /* root/detached: abort (never returns) */
}

/* ---- channels ------------------------------------------------------------- */
/* Rewritten onto the scheduler's park/wake (design/0005: "a blocked coroutine
 * parks its G and frees its M"). The channel lock is a plain pthread mutex held
 * only for the brief critical section; blocking happens via `chan_block`, not a
 * condvar wait on that mutex. Unbuffered rendezvous + buffered + close/comma-ok
 * semantics are identical to #25. */
typedef struct {
    pthread_mutex_t mu;
    uint64_t *buf; /* ring buffer (buffered); NULL when unbuffered */
    int64_t cap;   /* 0 => unbuffered rendezvous */
    int64_t count;
    int64_t head;
    int64_t tail;
    int closed;
    Waiter *sendq_head, *sendq_tail; /* senders waiting (val = offered value) */
    Waiter *recvq_head, *recvq_tail; /* receivers waiting (to be filled)      */
    uint8_t elem_ptr; /* buffered element words are GC pointers */
} ZapChan;

static void q_push(Waiter **head, Waiter **tail, Waiter *w) {
    w->next = NULL;
    if (*tail) {
        (*tail)->next = w;
    } else {
        *head = w;
    }
    *tail = w;
}
static Waiter *q_pop(Waiter **head, Waiter **tail) {
    Waiter *w = *head;
    if (w) {
        *head = w->next;
        if (!*head) {
            *tail = NULL;
        }
        w->next = NULL;
    }
    return w;
}

/* Wake a waiter the current thread has just resolved (its outcome is already
 * written). Called holding the channel lock. */
static void goready(Waiter *w) {
    if (w->g) {
        sched_enqueue(w->g);
    } else {
        w->done = 1;
        pthread_cond_signal(&w->cv);
    }
}

/* Block the current execution context on channel `c` (lock held on entry). The
 * waiter `w` is already enqueued on the relevant channel queue. Releases the
 * channel lock while blocked and guarantees it is released on return; the
 * waiter's outcome has been written by the waker. A goroutine parks its G and
 * frees its M; a non-goroutine (main) thread blocks on the waiter's condvar. */
static void chan_block(ZapChan *c, Waiter *w) {
    M *m = cur_m;
    if (m && m->current) {
        G *g = m->current;
        /* Soft stack-overflow guard at this scheduling point (no hardware guard
         * page under conservative GC; see the section header). */
        char probe;
        if ((char *)&probe - (char *)g->stack < ZAP_STACK_REDZONE) {
            pthread_mutex_unlock(&c->mu);
            zap_panic(zap_str_new("stack overflow", 14));
            return;
        }
        w->g = g;
        g->state = G_PARKED;
        g->parkmu = &c->mu; /* scheduler unlocks once we are fully parked */
        swapcontext(&g->ctx, &m->sched_ctx);
        /* Resumed (possibly on a different M); channel lock already released. */
    } else {
        w->g = NULL;
        pthread_cond_init(&w->cv, NULL);
        w->done = 0;
        /* Precise GC (#48): a native (main) thread blocked here is GC-safe —
         * its roots were flushed when it entered the blocking region, so a
         * collection need not wait for it. */
        zap_enter_blocking();
        while (!w->done) {
            pthread_cond_wait(&w->cv, &c->mu);
        }
        pthread_mutex_unlock(&c->mu);
        zap_leave_blocking();
    }
}

void *zap_chan_new(int64_t cap) {
    ZapChan *c = (ZapChan *)zap_obj(&zap_desc_chan);
    pthread_mutex_init(&c->mu, NULL);
    c->cap = cap > 0 ? cap : 0;
    c->buf = c->cap > 0
                 ? (uint64_t *)zap_ptrbuf((int64_t)c->cap * (int64_t)sizeof(uint64_t))
                 : NULL;
    c->count = c->head = c->tail = 0;
    c->closed = 0;
    c->sendq_head = c->sendq_tail = NULL;
    c->recvq_head = c->recvq_tail = NULL;
    return c;
}

static void chan_send_word(ZapChan *c, uint64_t v) {
    /* A channel op is a cancellation point (design/0005): if this coroutine's
     * scope was cancelled, raise `Cancelled` rather than proceeding/blocking. */
    zap_check_cancel();
    pthread_mutex_lock(&c->mu);
    if (c->closed) {
        pthread_mutex_unlock(&c->mu);
        zap_panic(zap_str_new("send on closed channel", 22));
        return;
    }
    /* A waiting receiver takes the value directly (rendezvous / fast path). */
    if (c->count == 0 && c->recvq_head) {
        Waiter *r = q_pop(&c->recvq_head, &c->recvq_tail);
        r->val = v;
        r->ok = 1;
        goready(r);
        pthread_mutex_unlock(&c->mu);
        return;
    }
    if (c->cap > 0 && c->count < c->cap) {
        c->buf[c->tail] = v;
        c->tail = (c->tail + 1) % c->cap;
        c->count++;
        pthread_mutex_unlock(&c->mu);
        return;
    }
    /* Would block: unbuffered with no receiver, or buffer full. Park as a
     * sender; the matching receiver delivers/buffers our value and wakes us. */
    Waiter w;
    w.val = v;
    w.ok = 0;
    w.closed = 0;
    q_push(&c->sendq_head, &c->sendq_tail, &w);
    chan_block(c, &w); /* channel lock released inside */
    if (w.closed) {
        zap_panic(zap_str_new("send on closed channel", 22));
    }
}

static uint64_t chan_recv_word(ZapChan *c, int *ok) {
    zap_check_cancel(); /* channel op is a cancellation point (design/0005) */
    pthread_mutex_lock(&c->mu);
    /* Buffered value available: take it, then let a waiting sender refill. */
    if (c->count > 0) {
        uint64_t v = c->buf[c->head];
        c->head = (c->head + 1) % c->cap;
        c->count--;
        Waiter *s = q_pop(&c->sendq_head, &c->sendq_tail);
        if (s) {
            c->buf[c->tail] = s->val;
            c->tail = (c->tail + 1) % c->cap;
            c->count++;
            s->ok = 1;
            goready(s);
        }
        pthread_mutex_unlock(&c->mu);
        if (ok) {
            *ok = 1;
        }
        return v;
    }
    /* A waiting sender hands its value over directly (rendezvous). */
    if (c->sendq_head) {
        Waiter *s = q_pop(&c->sendq_head, &c->sendq_tail);
        uint64_t v = s->val;
        s->ok = 1;
        goready(s);
        pthread_mutex_unlock(&c->mu);
        if (ok) {
            *ok = 1;
        }
        return v;
    }
    if (c->closed) {
        pthread_mutex_unlock(&c->mu);
        if (ok) {
            *ok = 0;
        }
        return 0;
    }
    /* Would block: park as a receiver; a sender (or close) wakes us. */
    Waiter w;
    w.val = 0;
    w.ok = 0;
    w.closed = 0;
    q_push(&c->recvq_head, &c->recvq_tail, &w);
    chan_block(c, &w); /* channel lock released inside */
    if (ok) {
        *ok = w.ok;
    }
    return w.ok ? w.val : 0;
}

void zap_chan_send_i64(void *ch, int64_t v) {
    chan_send_word((ZapChan *)ch, (uint64_t)v);
}
void zap_chan_send_ptr(void *ch, void *v) {
    ((ZapChan *)ch)->elem_ptr = 1;
    chan_send_word((ZapChan *)ch, (uint64_t)(uintptr_t)v);
}
int64_t zap_chan_recv_i64(void *ch) {
    int ok;
    return (int64_t)chan_recv_word((ZapChan *)ch, &ok);
}
void *zap_chan_recv_ptr(void *ch) {
    ((ZapChan *)ch)->elem_ptr = 1;
    int ok;
    return (void *)(uintptr_t)chan_recv_word((ZapChan *)ch, &ok);
}

void zap_chan_close(void *ch) {
    ZapChan *c = (ZapChan *)ch;
    pthread_mutex_lock(&c->mu);
    c->closed = 1;
    /* Waiting senders fail (send on closed); waiting receivers drain to the
     * closed zero-value with ok == 0. */
    Waiter *w;
    while ((w = q_pop(&c->sendq_head, &c->sendq_tail)) != NULL) {
        w->closed = 1;
        w->ok = 0;
        goready(w);
    }
    while ((w = q_pop(&c->recvq_head, &c->recvq_tail)) != NULL) {
        w->ok = 0;
        w->val = 0;
        goready(w);
    }
    pthread_mutex_unlock(&c->mu);
}

/* ===========================================================================
 * Type descriptors for the runtime's own heap types (ticket #48). Both GC
 * builds read `.size`; the precise Immix collector also reads `.kind` and the
 * pointer-offset arrays. Codegen emits the descriptors for *user* structs and
 * closures; these cover the built-in runtime objects. See design/0008-precise-gc.
 * ========================================================================== */
#include <stddef.h> /* offsetof */

static const int32_t zap_off_strobj[] = {(int32_t)offsetof(ZapStr, data)};
static const int32_t zap_off_gobj[] = {
    (int32_t)offsetof(G, stack),
    (int32_t)offsetof(G, arg),
    (int32_t)offsetof(G, scope),
};

const ZapTypeDesc zap_desc_atomic = {0, ZAP_TD_ATOMIC, 0, NULL};
const ZapTypeDesc zap_desc_ptrs = {0, ZAP_TD_ATOMIC, 0, NULL};
const ZapTypeDesc zap_desc_gstack = {0, ZAP_TD_ATOMIC, 0, NULL};
const ZapTypeDesc zap_desc_list = {(int64_t)sizeof(ZapList), ZAP_TD_LIST, 0, NULL};
const ZapTypeDesc zap_desc_dict = {(int64_t)sizeof(ZapDict), ZAP_TD_DICT, 0, NULL};
const ZapTypeDesc zap_desc_set = {(int64_t)sizeof(ZapSet), ZAP_TD_SET, 0, NULL};
const ZapTypeDesc zap_desc_heap = {(int64_t)sizeof(ZapHeap), ZAP_TD_HEAP, 0, NULL};
const ZapTypeDesc zap_desc_chan = {(int64_t)sizeof(ZapChan), ZAP_TD_CHAN, 0, NULL};
const ZapTypeDesc zap_desc_array = {(int64_t)sizeof(ZapArray), ZAP_TD_ARRAY, 0, NULL};
const ZapTypeDesc zap_desc_matchobj = {(int64_t)sizeof(ZapMatch), ZAP_TD_MATCH, 0, NULL};
const ZapTypeDesc zap_desc_scope = {(int64_t)sizeof(Scope), ZAP_TD_SCOPE, 0, NULL};
/* ZapPy wraps a borrowed CPython reference, not a GC pointer, so it is a leaf. */
const ZapTypeDesc zap_desc_pyobj = {(int64_t)sizeof(ZapPy), ZAP_TD_ATOMIC, 0, NULL};
const ZapTypeDesc zap_desc_strobj = {(int64_t)sizeof(ZapStr), ZAP_TD_PRECISE, 1,
                                     zap_off_strobj};
const ZapTypeDesc zap_desc_gobj = {(int64_t)sizeof(G), ZAP_TD_PRECISE, 3, zap_off_gobj};

#ifdef ZAP_GC_IMMIX
/* Custom root: the reflection registry is a static table (not a heap object)
 * holding ZapStr* names, so it must be enumerated explicitly at each GC. */
static void zap_reflect_mark_roots(void) {
    ZapReflectTable *t = &zap_reflect_table;
    if (t->items) {
        zap_gc_mark(t->items);
        for (int64_t i = 0; i < t->len; i++) {
            zap_gc_mark(t->items[i].type_name);
            zap_gc_mark(t->items[i].deco_name);
        }
    }
}

/* Register the precise collector's static roots once at startup. */
__attribute__((constructor)) static void zap_gc_register_statics(void) {
    zap_gc_add_root((void **)&g_cancelled);
    zap_gc_add_root_fn(zap_reflect_mark_roots);
}

/* The precise Immix collector implementation. Included last so it sees every
 * runtime type and the scheduler globals above. */
#include "zapgc.inc.c"
#endif
