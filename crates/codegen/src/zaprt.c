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
extern const ZapTypeDesc zap_desc_sigsub;  /* SigSub: {next, chan} (ticket #77)  */

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
/* Finalizers (ticket #92): register cleanup for an object, run once when it
 * becomes unreachable. `zap_py_wrap` uses it for the CPython-ref DECREF. */
void zap_gc_register_finalizer(void *payload, void (*fn)(void *obj, void *cd), void *cd);
int64_t zap_gc_finalizer_selftest(void); /* internal __gc_finalizer_selftest hook */
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
/* Finalizers (ticket #92): forward to Boehm's registration so the one call site
 * (`zap_py_wrap`) is backend-uniform. Boehm's default finalizer already provides
 * exactly these semantics (one-shot, run when unreachable, outside the GC). */
static inline void zap_gc_register_finalizer(void *payload,
                                             void (*fn)(void *obj, void *cd), void *cd) {
    GC_register_finalizer(payload, (GC_finalization_proc)fn, cd, NULL, NULL);
}
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
 *   - zap_gc_count(): number of collections run so far (GC cycle count).
 * The Immix implementations live in zapgc.inc.c (included below); the Boehm
 * ones wrap bdwgc here. */
int64_t zap_gc_count(void);
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
int64_t zap_gc_count(void) { return (int64_t)GC_get_gc_no(); }
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
 * `zap_py_wrap`); a finalizer DECREFs when the wrapper is collected — under Boehm
 * via GC_register_finalizer, under the precise Immix collector via its post-mark
 * finalizer queue (ticket #92), through the one backend-uniform
 * `zap_gc_register_finalizer` call site. The
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
    /* Backend-uniform: Boehm forwards to GC_register_finalizer; the precise Immix
     * collector runs this via its own post-mark finalizer queue (ticket #92). */
    zap_gc_register_finalizer(h, zap_py_finalize, NULL);
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
 * The netpoller (ticket #80) now parks a G blocked on a non-ready fd and frees
 * its M, re-queuing the G on readiness (see the "Netpoller" section below); it
 * reuses the `gopark`/`goready` primitive here.
 *
 * DEFERRED (documented in the PR / OPEN-QUESTIONS; semantics unchanged): the
 * blocking-syscall P-handoff, preemption of long CPU loops (v1 is
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
static void netpoll_shutdown(void);   /* fwd (netpoller section, below) */
static void zap_signal_shutdown(void); /* fwd (signals section, #77, below) */
void zap_rt_join_all(void) {
    if (!g_ms) {
        /* main may have used signals / the netpoller without ever spawning. Stop
         * the signal thread first (it parks ON the netpoller), then the poller. */
        zap_signal_shutdown();
        netpoll_shutdown();
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
    /* Every spawned G has finished: stop the signal thread (parked on the
     * netpoller self-pipe) and then the netpoller thread (both no-ops if never
     * started). */
    zap_signal_shutdown();
    netpoll_shutdown();
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
 * than one that next reaches a cancellation point) is not yet wired to `scope:`
 * cancellation, though the netpoller (ticket #80) now provides the
 * `zap_netpoll_wake` hook a later increment can build it on; parking — rather
 * than blocking the worker M — at a join reached from inside a coroutine (a
 * nested `scope:`) likewise remains a deferral. Both work at the default
 * MAXPROCS and the shapes the goldens exercise. */

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
void zap_scope_disarm(void *scope_); /* fwd (signals section, #77) */
void zap_scope_exit(void *scope_) {
    Scope *sc = (Scope *)scope_;
    scope_join(sc);
    /* The scope's lifetime ends here; drop any `cancel_on` subscription (ticket
     * #77) AFTER the join, so a signal can still cancel during the join. A no-op
     * for a scope with no cancel_on. */
    zap_scope_disarm(sc);
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
    zap_scope_disarm(sc); /* drop any cancel_on subscription (ticket #77) */
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

/* Park the current execution context on a scheduler-keyed wait queue guarded by
 * `mu` (held on entry). The waiter `w` is already enqueued on the relevant queue
 * by the caller. Releases `mu` while blocked and guarantees it is released on
 * return; the waiter's outcome has been written by the waker before `goready`.
 * A goroutine parks its G and frees its M (the scheduler unlocks `mu` once the G
 * is fully switched out, which closes the park/wake race); a non-goroutine
 * (main) thread blocks on the waiter's own condvar without consuming a worker M.
 *
 * This is the one shared park primitive the whole runtime blocks on — channels
 * (`chan_block`), scope joins, and the netpoller (ticket #80) all reuse it, so a
 * blocked coroutine is always parked rather than tying up an M. */
static void gopark(pthread_mutex_t *mu, Waiter *w) {
    M *m = cur_m;
    if (m && m->current) {
        G *g = m->current;
        /* Soft stack-overflow guard at this scheduling point (no hardware guard
         * page under conservative GC; see the section header). */
        char probe;
        if ((char *)&probe - (char *)g->stack < ZAP_STACK_REDZONE) {
            pthread_mutex_unlock(mu);
            zap_panic(zap_str_new("stack overflow", 14));
            return;
        }
        w->g = g;
        g->state = G_PARKED;
        g->parkmu = mu; /* scheduler unlocks once we are fully parked */
        swapcontext(&g->ctx, &m->sched_ctx);
        /* Resumed (possibly on a different M); `mu` already released. */
    } else {
        w->g = NULL;
        pthread_cond_init(&w->cv, NULL);
        w->done = 0;
        /* Precise GC (#48): a native (main) thread blocked here is GC-safe —
         * its roots were flushed when it entered the blocking region, so a
         * collection need not wait for it. */
        zap_enter_blocking();
        while (!w->done) {
            pthread_cond_wait(&w->cv, mu);
        }
        pthread_mutex_unlock(mu);
        zap_leave_blocking();
    }
}

/* Block the current execution context on channel `c` (lock held on entry); a
 * thin wrapper over the shared `gopark` primitive keyed on the channel lock. */
static void chan_block(ZapChan *c, Waiter *w) { gopark(&c->mu, w); }

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
 * Netpoller — non-blocking I/O readiness for the M:N scheduler
 * (ticket #80, D-0008, design/0005-concurrency.md "Blocking syscalls").
 *
 * A goroutine that would block waiting for a file descriptor to become readable
 * or writable registers the fd here and PARKS its G (freeing its M to run other
 * goroutines) instead of blocking the OS thread in a read/write syscall. A
 * dedicated poller OS thread waits on the readiness mechanism (epoll on Linux)
 * and, when an fd is ready, re-queues the parked G on the scheduler. This is the
 * Go netpoller analogue and the deferral called out in #47's PR.
 *
 * It reuses the scheduler's ONE park/wake primitive (`gopark` / `goready`),
 * exactly as channels do — a parked reader frees its M; a native (main) thread
 * caller blocks on its waiter's condvar without consuming a worker M. No
 * parallel blocking mechanism is introduced.
 *
 * GC (both collectors): a `PollDesc` is plain `malloc` infra memory, never a GC
 * object, so it needs no tracing; the G it references while parked is kept alive
 * independently by the statically-rooted `g_all[]` registry (and under Immix is
 * traced via its shadow chain), just like a channel-parked G. The poller thread
 * allocates no GC memory and touches only non-traced scheduler fields (run-queue
 * links / state) under their mutexes, so it is safe without being a registered
 * mutator; under Boehm it is started with `GC_pthread_create` so the collector
 * still knows the thread.
 *
 * Design choices (flagged for review per the ticket):
 *   - EDGE-TRIGGERED (EPOLLET), Go-style: the fd is registered once for both
 *     read and write interest; a readiness edge that arrives before any G parks
 *     is LATCHED per direction so the next `wait` returns immediately. Callers
 *     must drain to EAGAIN (the standard ET contract; documented in RUNTIME-ABI).
 *   - Deadlines are absolute CLOCK_MONOTONIC nanoseconds (0 = none); the poller
 *     wakes an expired waiter with ZAP_POLL_TIMEOUT. A full timer heap is a v1
 *     follow-up — today the poller scans the (small) live-desc registry.
 *
 * PORTABILITY: epoll is the Linux/dev-container backend and the only one wired
 * for v1/CI. The readiness mechanism sits behind the thin `np_backend_*` seam
 * below so a kqueue backend (macOS/BSD) can slot in later without touching the
 * scheduler-facing logic — a deliberate portability follow-up, NOT built here.
 *
 * Internal C ABI (no language surface yet; sockets/http are #79, signals #77):
 *   void   *zap_netpoll_open(int fd)                 register fd, returns desc
 *   int64_t zap_netpoll_wait(desc, mode, deadline_ns) park until ready/deadline
 *   void    zap_netpoll_wake(desc, mode)             force-wake parked waiter(s)
 *   void    zap_netpoll_close(desc)                  deregister + free
 * `mode` is ZAP_POLL_READ / ZAP_POLL_WRITE; `wait` returns ZAP_POLL_OK /
 * _TIMEOUT / _CLOSED. `zap_netpoll_wake` is the hook the signal self-pipe (#77)
 * and a future cancellation-of-parked-G path build on.
 * ========================================================================== */

enum { ZAP_POLL_READ = 1, ZAP_POLL_WRITE = 2 };
enum { ZAP_POLL_OK = 0, ZAP_POLL_TIMEOUT = 1, ZAP_POLL_CLOSED = 2 };

typedef struct PollDesc {
    int fd;
    int closing;                 /* zap_netpoll_close has been called           */
    pthread_mutex_t mu;          /* guards the waiter slots / latches / deadlines */
    Waiter *rg;                  /* parked reader waiter, or NULL                */
    Waiter *wg;                  /* parked writer waiter, or NULL                */
    int r_ready;                 /* readiness edge latched with no reader parked  */
    int w_ready;
    int64_t r_deadline;          /* CLOCK_MONOTONIC ns, or 0 = none              */
    int64_t w_deadline;
    struct PollDesc *all_prev;   /* live-desc registry (deadline scan)           */
    struct PollDesc *all_next;
    struct PollDesc *free_link;  /* free list when idle                          */
} PollDesc;

/* Registry + free list + lifecycle lock. Lock order is ALWAYS g_np_mu then a
 * PollDesc's own mu; no path takes them the other way round. */
static pthread_mutex_t g_np_mu = PTHREAD_MUTEX_INITIALIZER;
static PollDesc *g_np_all = NULL;   /* doubly-linked list of live descriptors   */
static PollDesc *g_np_free = NULL;  /* recycled descriptors                      */

static pthread_once_t g_np_once = PTHREAD_ONCE_INIT;
static volatile int g_np_started = 0;
static volatile int g_np_shutdown = 0;
static pthread_t g_np_thread;

static int64_t np_now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}

/* ---- backend seam (epoll today; kqueue is the portability follow-up) ------ */
#if defined(__linux__)
#define ZAP_NETPOLL_EPOLL 1
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <fcntl.h>
#include <errno.h>

static int g_np_epfd = -1;   /* epoll instance                                  */
static int g_np_wakefd = -1; /* eventfd to interrupt epoll_wait (kick/shutdown) */
static char g_np_wake_tag;   /* sentinel: epoll_event.data.ptr for the wake fd  */

static int np_backend_open(void) {
    g_np_epfd = epoll_create1(EPOLL_CLOEXEC);
    if (g_np_epfd < 0) {
        return -1;
    }
    g_np_wakefd = eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK);
    if (g_np_wakefd < 0) {
        close(g_np_epfd);
        g_np_epfd = -1;
        return -1;
    }
    struct epoll_event ev;
    memset(&ev, 0, sizeof ev);
    ev.events = EPOLLIN; /* level-triggered wake fd: always drained below       */
    ev.data.ptr = &g_np_wake_tag;
    if (epoll_ctl(g_np_epfd, EPOLL_CTL_ADD, g_np_wakefd, &ev) < 0) {
        return -1;
    }
    return 0;
}

static int np_backend_add(PollDesc *pd) {
    struct epoll_event ev;
    memset(&ev, 0, sizeof ev);
    /* One registration serves both directions; edge-triggered (see header). */
    ev.events = EPOLLIN | EPOLLOUT | EPOLLRDHUP | EPOLLET;
    ev.data.ptr = pd;
    return epoll_ctl(g_np_epfd, EPOLL_CTL_ADD, pd->fd, &ev);
}

static void np_backend_del(PollDesc *pd) {
    epoll_ctl(g_np_epfd, EPOLL_CTL_DEL, pd->fd, NULL);
}

static void np_backend_kick(void) {
    uint64_t one = 1;
    ssize_t r = write(g_np_wakefd, &one, sizeof one);
    (void)r; /* a full counter (EAGAIN) still wakes epoll_wait */
}
#else
/* No readiness backend on this platform (kqueue follow-up): the netpoller
 * degrades to "unsupported" — zap_netpoll_open returns NULL. zaprt.c is only
 * compiled on Linux (the LLVM dev container / CI), so this path is a stub. */
static int g_np_epfd = -1;
static int np_backend_open(void) { return -1; }
static int np_backend_add(PollDesc *pd) { (void)pd; return -1; }
static void np_backend_del(PollDesc *pd) { (void)pd; }
static void np_backend_kick(void) {}
#endif

/* ---- generic poller (backend-agnostic) ------------------------------------ */
/* Deliver readiness/timeout/closed to one direction of `pd` (pd->mu held). A
 * parked waiter is popped, given its outcome, and re-queued (`goready`); a bare
 * readiness edge with no waiter is LATCHED so the next `wait` consumes it. */
static void np_wake_dir_locked(PollDesc *pd, int is_read, int status) {
    Waiter **slot = is_read ? &pd->rg : &pd->wg;
    int *ready = is_read ? &pd->r_ready : &pd->w_ready;
    int64_t *dl = is_read ? &pd->r_deadline : &pd->w_deadline;
    if (*slot) {
        Waiter *w = *slot;
        *slot = NULL;
        *dl = 0;
        w->val = (uint64_t)status;
        goready(w); /* enqueue the parked G, or signal a native waiter's condvar */
    } else if (status == ZAP_POLL_OK) {
        *ready = 1; /* edge arrived before anyone parked: latch it (edge-triggered) */
    }
}

static void np_all_remove_locked(PollDesc *pd) { /* caller holds g_np_mu */
    if (pd->all_prev) {
        pd->all_prev->all_next = pd->all_next;
    } else {
        g_np_all = pd->all_next;
    }
    if (pd->all_next) {
        pd->all_next->all_prev = pd->all_prev;
    }
    pd->all_prev = pd->all_next = NULL;
}

/* epoll_wait timeout (ms) = time until the nearest pending deadline, -1 if none.
 * Capped at 1h so a far-future deadline still re-polls periodically. */
static int np_compute_timeout(void) {
    int have = 0;
    int64_t best = 0;
    pthread_mutex_lock(&g_np_mu);
    for (PollDesc *pd = g_np_all; pd; pd = pd->all_next) {
        pthread_mutex_lock(&pd->mu);
        int64_t d = 0;
        if (pd->rg && pd->r_deadline) {
            d = pd->r_deadline;
        }
        if (pd->wg && pd->w_deadline && (d == 0 || pd->w_deadline < d)) {
            d = pd->w_deadline;
        }
        pthread_mutex_unlock(&pd->mu);
        if (d && (!have || d < best)) {
            best = d;
            have = 1;
        }
    }
    pthread_mutex_unlock(&g_np_mu);
    if (!have) {
        return -1;
    }
    int64_t now = np_now_ns();
    if (best <= now) {
        return 0;
    }
    int64_t ms = (best - now + 999999) / 1000000;
    if (ms > 3600000) {
        ms = 3600000;
    }
    return (int)ms;
}

/* Wake every waiter whose deadline has passed (called after each poll wait). */
static void np_expire_deadlines(void) {
    int64_t now = np_now_ns();
    pthread_mutex_lock(&g_np_mu);
    for (PollDesc *pd = g_np_all; pd; pd = pd->all_next) {
        pthread_mutex_lock(&pd->mu);
        if (pd->rg && pd->r_deadline && pd->r_deadline <= now) {
            Waiter *w = pd->rg;
            pd->rg = NULL;
            pd->r_deadline = 0;
            w->val = ZAP_POLL_TIMEOUT;
            goready(w);
        }
        if (pd->wg && pd->w_deadline && pd->w_deadline <= now) {
            Waiter *w = pd->wg;
            pd->wg = NULL;
            pd->w_deadline = 0;
            w->val = ZAP_POLL_TIMEOUT;
            goready(w);
        }
        pthread_mutex_unlock(&pd->mu);
    }
    pthread_mutex_unlock(&g_np_mu);
}

#if defined(ZAP_NETPOLL_EPOLL)
static void *np_poller_main(void *arg) {
    (void)arg;
    struct epoll_event evs[64];
    for (;;) {
        int timeout = np_compute_timeout();
        int n = epoll_wait(g_np_epfd, evs, 64, timeout);
        if (g_np_shutdown) {
            break;
        }
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            break; /* unexpected epoll failure: stop polling */
        }
        for (int i = 0; i < n; i++) {
            void *p = evs[i].data.ptr;
            if (p == &g_np_wake_tag) {
                uint64_t buf;
                ssize_t r = read(g_np_wakefd, &buf, sizeof buf);
                (void)r; /* just a kick; drain and recompute the timeout */
                continue;
            }
            PollDesc *pd = (PollDesc *)p;
            uint32_t e = evs[i].events;
            pthread_mutex_lock(&pd->mu);
            /* An error/hangup counts as readiness for BOTH directions so the
             * parked G wakes and observes the condition via its next syscall. */
            if (e & (EPOLLIN | EPOLLERR | EPOLLHUP | EPOLLRDHUP)) {
                np_wake_dir_locked(pd, 1, ZAP_POLL_OK);
            }
            if (e & (EPOLLOUT | EPOLLERR | EPOLLHUP)) {
                np_wake_dir_locked(pd, 0, ZAP_POLL_OK);
            }
            pthread_mutex_unlock(&pd->mu);
        }
        np_expire_deadlines();
    }
    return NULL;
}
#endif

/* ---- lazy start / shutdown ------------------------------------------------ */
static void np_start_once(void) {
#if defined(ZAP_NETPOLL_EPOLL)
    if (np_backend_open() != 0) {
        fprintf(stderr, "zap: netpoller init failed (epoll)\n");
        return; /* g_np_started stays 0 => zap_netpoll_open reports unsupported */
    }
    int rc = GC_pthread_create(&g_np_thread, NULL, np_poller_main, NULL);
    if (rc != 0) {
        fprintf(stderr, "zap: netpoller thread start failed (%d)\n", rc);
        return;
    }
    g_np_started = 1;
#endif
}

static int np_ensure(void) {
    pthread_once(&g_np_once, np_start_once);
    return g_np_started ? 0 : -1;
}

/* Stop the poller thread. Called from zap_rt_join_all at program exit, after
 * every spawned G has finished (so nothing is parked on an fd). No-op if the
 * poller never started. */
static void netpoll_shutdown(void) {
    if (!g_np_started) {
        return;
    }
    g_np_shutdown = 1;
    np_backend_kick();
    GC_pthread_join(g_np_thread, NULL);
    g_np_started = 0;
}

/* ---- descriptor lifecycle ------------------------------------------------- */
static PollDesc *np_pd_alloc(int fd) {
    pthread_mutex_lock(&g_np_mu);
    PollDesc *pd = g_np_free;
    if (pd) {
        g_np_free = pd->free_link;
    }
    pthread_mutex_unlock(&g_np_mu);
    if (!pd) {
        pd = (PollDesc *)malloc(sizeof(PollDesc));
        if (!pd) {
            return NULL;
        }
        pthread_mutex_init(&pd->mu, NULL);
    }
    pd->fd = fd;
    pd->closing = 0;
    pd->rg = pd->wg = NULL;
    pd->r_ready = pd->w_ready = 0;
    pd->r_deadline = pd->w_deadline = 0;
    pd->all_prev = pd->all_next = pd->free_link = NULL;
    return pd;
}

/* ---- public internal ABI -------------------------------------------------- */
/* Register `fd` for readiness polling; returns an opaque descriptor, or NULL if
 * the netpoller is unavailable (non-Linux v1) or registration failed. The caller
 * owns `fd` and should have set it non-blocking. */
void *zap_netpoll_open(int64_t fd) {
    if (np_ensure() != 0) {
        return NULL;
    }
    PollDesc *pd = np_pd_alloc((int)fd);
    if (!pd) {
        return NULL;
    }
    pthread_mutex_lock(&g_np_mu);
    pd->all_prev = NULL;
    pd->all_next = g_np_all;
    if (g_np_all) {
        g_np_all->all_prev = pd;
    }
    g_np_all = pd;
    pthread_mutex_unlock(&g_np_mu);
    if (np_backend_add(pd) != 0) {
        pthread_mutex_lock(&g_np_mu);
        np_all_remove_locked(pd);
        pd->free_link = g_np_free;
        g_np_free = pd;
        pthread_mutex_unlock(&g_np_mu);
        return NULL;
    }
    return pd;
}

/* Park the current G (or native thread) until `fd` is ready for `mode`
 * (ZAP_POLL_READ / ZAP_POLL_WRITE), its `deadline_ns` (absolute CLOCK_MONOTONIC;
 * 0 = none) passes, or it is closed. Returns ZAP_POLL_OK / _TIMEOUT / _CLOSED.
 * A parked G frees its M (netpoller's whole point). */
int64_t zap_netpoll_wait(void *pd_, int64_t mode, int64_t deadline_ns) {
    PollDesc *pd = (PollDesc *)pd_;
    int is_read = (mode == ZAP_POLL_READ);
    pthread_mutex_lock(&pd->mu);
    if (pd->closing) {
        pthread_mutex_unlock(&pd->mu);
        return ZAP_POLL_CLOSED;
    }
    int *ready = is_read ? &pd->r_ready : &pd->w_ready;
    if (*ready) {
        *ready = 0; /* consume a latched readiness edge without parking */
        pthread_mutex_unlock(&pd->mu);
        return ZAP_POLL_OK;
    }
    if (deadline_ns && deadline_ns <= np_now_ns()) {
        pthread_mutex_unlock(&pd->mu);
        return ZAP_POLL_TIMEOUT;
    }
    Waiter w;
    w.val = ZAP_POLL_OK;
    w.ok = 0;
    w.closed = 0;
    if (is_read) {
        pd->rg = &w;
        pd->r_deadline = deadline_ns;
    } else {
        pd->wg = &w;
        pd->w_deadline = deadline_ns;
    }
    if (deadline_ns) {
        np_backend_kick(); /* make the poller recompute its timeout */
    }
    gopark(&pd->mu, &w); /* releases pd->mu; the poller wakes us */
    return (int64_t)w.val;
}

/* Force-wake the waiter parked on `pd` for `mode` with ZAP_POLL_OK; if none is
 * parked, LATCH readiness so the next `zap_netpoll_wait` for that direction
 * returns immediately (same edge-latch np_wake_dir_locked uses). The latch is
 * what makes this race-free: a caller (e.g. the #77 signal-thread shutdown)
 * that wakes while the waiter is momentarily between waits — not yet re-parked —
 * must not have its wake silently dropped, or the waiter blocks forever. The
 * hook the signal self-pipe (#77) and a future cancellation-of-parked-G path
 * use to interrupt a netpoller wait. */
void zap_netpoll_wake(void *pd_, int64_t mode) {
    PollDesc *pd = (PollDesc *)pd_;
    pthread_mutex_lock(&pd->mu);
    if (mode & ZAP_POLL_READ) {
        if (pd->rg) {
            Waiter *w = pd->rg;
            pd->rg = NULL;
            pd->r_deadline = 0;
            w->val = ZAP_POLL_OK;
            goready(w);
        } else {
            pd->r_ready = 1; /* latch: the next read wait returns at once */
        }
    }
    if (mode & ZAP_POLL_WRITE) {
        if (pd->wg) {
            Waiter *w = pd->wg;
            pd->wg = NULL;
            pd->w_deadline = 0;
            w->val = ZAP_POLL_OK;
            goready(w);
        } else {
            pd->w_ready = 1; /* latch: the next write wait returns at once */
        }
    }
    pthread_mutex_unlock(&pd->mu);
}

/* Deregister `fd` and release the descriptor. Any waiter still parked is woken
 * with ZAP_POLL_CLOSED. The caller still owns (and should close) the fd itself. */
void zap_netpoll_close(void *pd_) {
    PollDesc *pd = (PollDesc *)pd_;
    pthread_mutex_lock(&g_np_mu);
    pthread_mutex_lock(&pd->mu);
    pd->closing = 1;
    np_backend_del(pd);
    if (pd->rg) {
        Waiter *w = pd->rg;
        pd->rg = NULL;
        w->val = ZAP_POLL_CLOSED;
        goready(w);
    }
    if (pd->wg) {
        Waiter *w = pd->wg;
        pd->wg = NULL;
        w->val = ZAP_POLL_CLOSED;
        goready(w);
    }
    np_all_remove_locked(pd);
    pthread_mutex_unlock(&pd->mu);
    pd->free_link = g_np_free; /* recycle; a woken waiter never touches pd again */
    g_np_free = pd;
    pthread_mutex_unlock(&g_np_mu);
}

/* ---- integration self-test (ticket #80) ----------------------------------- *
 * The netpoller has no language-level I/O surface yet (sockets/http are #79,
 * signals #77), so this drives it end-to-end from C and backs the
 * `__netpoll_selftest()` golden (run under both collectors). It forces a SINGLE
 * worker M (`zap_sched_init(1)`) so the "a parked G frees its M" property is
 * load-bearing: a reader goroutine blocks on a not-yet-ready pipe and MUST park
 * (freeing the one M) or the writer goroutine could never be scheduled and the
 * program would deadlock. Returns the number of sub-checks that passed
 * (ZAP_NP_SELFTEST_CHECKS on full success); prints to stderr on any failure. */
#define ZAP_NP_SELFTEST_CHECKS 2

#if defined(ZAP_NETPOLL_EPOLL)
static int g_np_t_rfd;        /* read end polled by the reader goroutine        */
static int g_np_t_wfd;        /* write end the writer goroutine writes to       */
static void *g_np_t_pd;       /* its PollDesc                                   */
static void *g_np_t_done;     /* buffered channel the two goroutines signal on  */
static volatile int g_np_t_byte;   /* byte the reader received                  */
static volatile int g_np_t_status; /* reader's zap_netpoll_wait outcome         */

static void np_set_nonblock(int fd) {
    int fl = fcntl(fd, F_GETFL, 0);
    if (fl >= 0) {
        fcntl(fd, F_SETFL, fl | O_NONBLOCK);
    }
}

/* Reader goroutine: parks on the pipe until readable, then drains one byte. */
static void *np_selftest_reader(void *arg) {
    (void)arg;
    g_np_t_status = (int)zap_netpoll_wait(g_np_t_pd, ZAP_POLL_READ, 0);
    unsigned char b = 0;
    if (g_np_t_status == ZAP_POLL_OK) {
        ssize_t r = read(g_np_t_rfd, &b, 1);
        g_np_t_byte = (r == 1) ? (int)b : -1;
    } else {
        g_np_t_byte = -1;
    }
    zap_chan_send_i64(g_np_t_done, 1);
    return NULL;
}

/* Writer goroutine: writes the sentinel byte to wake the parked reader. Runs on
 * the freed M while the reader is parked (single-M => this is the proof). */
static void *np_selftest_writer(void *arg) {
    (void)arg;
    unsigned char b = 0x5A; /* 'Z' == 90 */
    ssize_t r = write(g_np_t_wfd, &b, 1);
    (void)r;
    zap_chan_send_i64(g_np_t_done, 1);
    return NULL;
}

int64_t zap_netpoll_selftest(void) {
    int passed = 0;

    /* Single worker M so a blocked-but-parked reader is the only way the writer
     * ever runs (see header). Safe if the scheduler is already up (no-op then). */
    zap_sched_init(1);

    /* --- check 1: park on a not-yet-ready fd, wake on readiness --- */
    int fds[2];
    if (pipe(fds) == 0) {
        g_np_t_rfd = fds[0];
        g_np_t_wfd = fds[1];
        np_set_nonblock(fds[0]);
        np_set_nonblock(fds[1]);
        g_np_t_pd = zap_netpoll_open(fds[0]);
        if (g_np_t_pd) {
            g_np_t_byte = 0;
            g_np_t_status = -1;
            g_np_t_done = zap_chan_new(2); /* buffered: senders never block */
            /* The done channel is reachable only via this static C slot, which
             * is not conservatively scanned under the precise collector: root it
             * so a collection mid-test cannot reclaim it (no-op under Boehm). */
            zap_gc_add_root(&g_np_t_done);
            zap_spawn(np_selftest_reader, NULL);
            zap_spawn(np_selftest_writer, NULL);
            /* Native wait for both goroutines (parks main on the channel). */
            (void)zap_chan_recv_i64(g_np_t_done);
            (void)zap_chan_recv_i64(g_np_t_done);
            if (g_np_t_status == ZAP_POLL_OK && g_np_t_byte == 90) {
                passed++;
            } else {
                fprintf(stderr,
                        "netpoll selftest: readiness check failed "
                        "(status=%d byte=%d)\n",
                        g_np_t_status, g_np_t_byte);
            }
            zap_netpoll_close(g_np_t_pd);
        } else {
            fprintf(stderr, "netpoll selftest: zap_netpoll_open failed\n");
        }
        close(fds[0]);
        close(fds[1]);
    } else {
        fprintf(stderr, "netpoll selftest: pipe() failed\n");
    }

    /* --- check 2: a deadline expires when the fd never becomes ready --- */
    int fds2[2];
    if (pipe(fds2) == 0) {
        np_set_nonblock(fds2[0]);
        np_set_nonblock(fds2[1]);
        void *pd2 = zap_netpoll_open(fds2[0]);
        if (pd2) {
            int64_t st = zap_netpoll_wait(pd2, ZAP_POLL_READ,
                                          np_now_ns() + 30000000LL /* 30 ms */);
            if (st == ZAP_POLL_TIMEOUT) {
                passed++;
            } else {
                fprintf(stderr, "netpoll selftest: deadline check failed (st=%lld)\n",
                        (long long)st);
            }
            zap_netpoll_close(pd2);
        } else {
            fprintf(stderr, "netpoll selftest: zap_netpoll_open(2) failed\n");
        }
        close(fds2[0]);
        close(fds2[1]);
    }

    return passed;
}
#else
int64_t zap_netpoll_selftest(void) { return ZAP_NP_SELFTEST_CHECKS; }
#endif

/* ===========================================================================
 * Signals, runtime diagnostics & fault reporting (ticket #77, D-0058)
 * ---------------------------------------------------------------------------
 * First-class Unix signals built on the concurrency runtime. The mechanism
 * (D-0058): an async-signal-safe handler on a DEDICATED signal thread writes the
 * signal number to a SELF-PIPE whose read end is REGISTERED WITH THE NETPOLLER
 * (ticket #80); the signal thread parks on the netpoller (`zap_netpoll_wait`,
 * the native-waiter path) and, on readiness, drains the pipe and DISPATCHES in
 * normal (non-handler) context:
 *   - fan-out to every `signal.notify()` subscriber channel (cap-1 buffered,
 *     coalescing — a dropped edge, not a queue);
 *   - arm any `scope(cancel_on=...)` whose signal fired (cooperative cancel of
 *     the scope tree, plus an optional `grace` hard-exit deadline);
 *   - run the built-in DEFAULT diagnostic handler for a signal that has no user
 *     subscriber (SIGQUIT -> stacks + runtime dump; SIGUSR1 -> GC + mem stats).
 *
 * The capability ships in every binary and wires up LAZILY: nothing starts until
 * the generated `main` prologue calls `zap_signal_init()` (which installs the
 * default handlers, ON by default — opt out at build with -DZAP_NO_DEFAULT_SIGNALS)
 * or the program first subscribes (`signal.notify`, `@on_signal`,
 * `scope(cancel_on=...)`). `notify` TAKES OVER a signal's disposition (the OS
 * default is suppressed; there is NO auto-exit — the program decides when to exit
 * after cleanup). Catchable signals only; SIGKILL/SIGSTOP cannot be caught.
 *
 * DEVIATION (flagged for review): D-0058 describes the reader as a separate
 * "dispatcher goroutine". Here the dedicated signal thread is itself the reader/
 * dispatcher — it still parks on the netpoller-registered self-pipe and delivers
 * in normal context, and fan-out is a non-blocking, non-allocating channel offer
 * that is scheduler-safe from a native (non-M) thread. A distinct dispatcher
 * goroutine would force the M:N worker pool onto EVERY binary (default handlers
 * are on by default), breaking the deliberate scheduler-laziness invariant that
 * keeps non-concurrent programs zero-cost (#47). `@on_signal` delivery DOES run
 * on a real goroutine (a detached receiver loop over the notify channel).
 *
 * FAULT signals (SIGSEGV/SIGFPE/SIGBUS/SIGILL) are NOT delivered via `notify`: a
 * crash handler (on a sigaltstack, so a stack overflow is still reportable)
 * prints the faulting signal/address, the offending goroutine, and a native
 * backtrace, then re-raises the default disposition so the process dies with the
 * right status (and a core, if enabled).
 * =========================================================================== */
#include <signal.h>
#include <fcntl.h>
#include <errno.h>
#if defined(__GLIBC__)
#include <execinfo.h> /* backtrace() for the fault crash report                 */
#define ZAP_HAVE_BACKTRACE 1
#endif

#define ZAP_NSIG 65 /* table span: standard + a margin of RT signals (Linux)    */
/* Fixed alternate-stack size: SIGSTKSZ is not a compile-time constant on recent
 * glibc, so size the crash-handler stack explicitly (well above MINSIGSTKSZ). */
#define ZAP_ALTSTACK_SIZE (64 * 1024)

/* A `signal.notify()` subscriber: a cap-1 coalescing channel. Nodes are GC
 * objects (precise desc {next, chan}) kept reachable by a registered root
 * marker under Immix and by conservative scan of the static array under Boehm. */
typedef struct SigSub {
    struct SigSub *next; /* MUST be first two slots -> see zap_off_sigsub        */
    void *chan;          /* ZapChan* (cap 1); delivered the signal number (i64)  */
} SigSub;

/* A `scope(cancel_on=...)` subscription: arm `scope` when the signal fires.
 * Plain malloc (not GC): the owning `scope:` frame keeps the Scope rooted for
 * the whole lifetime of this subscription (enter..disarm), and the node holds a
 * refcount so a pending grace timer never dereferences freed memory. */
typedef struct ScopeSigSub {
    struct ScopeSigSub *next;
    Scope *scope;
    int64_t grace_ms;
    int completed; /* scope left its block (disarmed) before any hard-exit       */
    int refcnt;    /* 1 (list) + 1 per outstanding grace timer                   */
} ScopeSigSub;

static pthread_mutex_t g_sig_mu = PTHREAD_MUTEX_INITIALIZER;
static SigSub *g_sig_subs[ZAP_NSIG];        /* notify() subscribers, per signo   */
static ScopeSigSub *g_scope_subs[ZAP_NSIG]; /* cancel_on subscriptions, per signo */
static int g_sig_default[ZAP_NSIG];         /* 1 => run the built-in default      */
static int g_sig_installed[ZAP_NSIG];       /* OS handler installed for this sig  */

static int g_sig_pipe[2] = {-1, -1}; /* [0]=read (netpoller), [1]=write (handler)*/
static void *g_sig_pd = NULL;        /* PollDesc for the read end                */
static pthread_t g_sig_thread;
static volatile int g_sig_active = 0;    /* subsystem started                    */
static volatile int g_sig_shutdown = 0;  /* signal thread asked to stop          */
static pthread_once_t g_sig_once = PTHREAD_ONCE_INIT;
static volatile sig_atomic_t g_sig_delivered = 0; /* async handler run count (test sync) */

/* Drain barrier for deterministic tests (__signal_drain): a sentinel byte (0,
 * never a real signal number) travels the self-pipe behind any pending edges;
 * the signal thread bumps g_sig_sync_seq when it processes one, FIFO order
 * guaranteeing every earlier edge was already dispatched. */
#define ZAP_SIG_SYNC_BYTE 0
static pthread_mutex_t g_sig_sync_mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t g_sig_sync_cv = PTHREAD_COND_INITIALIZER;
static int64_t g_sig_sync_seq = 0;

/* Human-readable signal name for diagnostics (no allocation; async-safe-ish). */
static const char *sig_name(int s) {
    switch (s) {
    case SIGHUP: return "SIGHUP";
    case SIGINT: return "SIGINT";
    case SIGQUIT: return "SIGQUIT";
    case SIGILL: return "SIGILL";
    case SIGABRT: return "SIGABRT";
    case SIGBUS: return "SIGBUS";
    case SIGFPE: return "SIGFPE";
    case SIGUSR1: return "SIGUSR1";
    case SIGSEGV: return "SIGSEGV";
    case SIGUSR2: return "SIGUSR2";
    case SIGPIPE: return "SIGPIPE";
    case SIGALRM: return "SIGALRM";
    case SIGTERM: return "SIGTERM";
    case SIGCHLD: return "SIGCHLD";
    case SIGCONT: return "SIGCONT";
    case SIGTSTP: return "SIGTSTP";
    case SIGWINCH: return "SIGWINCH";
    default: return "SIG?";
    }
}

/* ---- non-blocking, coalescing channel offer (delivery primitive) ---------- *
 * Hand `v` to a waiting receiver or buffer it; if neither is possible (buffer
 * full, or unbuffered with no receiver) DROP it — signals are edges, not a
 * queue (D-0058 "coalesces; drops on overflow"). Never blocks. Returns 1 if the
 * value was delivered/buffered, 0 if dropped. Mirrors chan_send_word's fast
 * paths without the parking tail. */
static int chan_offer_word(ZapChan *c, uint64_t v) {
    pthread_mutex_lock(&c->mu);
    if (c->closed) {
        pthread_mutex_unlock(&c->mu);
        return 0;
    }
    if (c->count == 0 && c->recvq_head) {
        Waiter *r = q_pop(&c->recvq_head, &c->recvq_tail);
        r->val = v;
        r->ok = 1;
        goready(r);
        pthread_mutex_unlock(&c->mu);
        return 1;
    }
    if (c->cap > 0 && c->count < c->cap) {
        c->buf[c->tail] = v;
        c->tail = (c->tail + 1) % c->cap;
        c->count++;
        pthread_mutex_unlock(&c->mu);
        return 1;
    }
    pthread_mutex_unlock(&c->mu);
    return 0; /* coalesce: drop this edge */
}

/* ---- diagnostics: goroutine / scheduler dump (SIGQUIT default, runtime.*) -- */
static const char *gstate_name(GState s) {
    switch (s) {
    case G_RUNNABLE: return "runnable";
    case G_RUNNING: return "running";
    case G_PARKED: return "parked";
    case G_DEAD: return "dead";
    default: return "?";
    }
}

/* Count live goroutines (runnable+running+parked). Reads the static g_all[]
 * registry under its lock; safe from any thread. */
int64_t zap_runtime_num_goroutines(void) {
    pthread_mutex_lock(&g_all_mu);
    int64_t n = g_all_len;
    pthread_mutex_unlock(&g_all_mu);
    return n;
}

/* Scalar GC byte getters that back `gc.stats() -> MemStats` field-by-field
 * (so the lowerer builds the struct from plain i64 calls, no out-params). All
 * route through the backend-agnostic zap_gc_stats. */
int64_t zap_gc_heap_bytes(void) {
    int64_t h = 0, l = 0;
    zap_gc_stats(&h, &l);
    return h;
}
int64_t zap_gc_live_bytes(void) {
    int64_t h = 0, l = 0;
    zap_gc_stats(&h, &l);
    return l;
}
int64_t zap_gc_free_bytes(void) {
    int64_t h = 0, l = 0;
    zap_gc_stats(&h, &l);
    return h > l ? h - l : 0;
}

/* Dump every live goroutine + a scheduler summary to `f`. v1 reports per-G
 * identity/state/ownership and the scheduler/GC shape; full per-G symbolicated
 * stack traces (unwinding each green stack) are a follow-up (noted in the PR). */
static void zap_goroutine_dump_to(FILE *f) {
    int64_t ng, maxprocs, live;
    pthread_mutex_lock(&g_live_mu);
    live = g_live;
    pthread_mutex_unlock(&g_live_mu);
    maxprocs = (int64_t)g_nm;
    fprintf(f, "=== zap runtime dump ===\n");
    fprintf(f, "GOMAXPROCS(M=P): %lld  root-live goroutines: %lld\n",
            (long long)maxprocs, (long long)live);
    int64_t heap = 0, lb = 0;
    zap_gc_stats(&heap, &lb);
    fprintf(f, "GC: cycles=%lld heap=%lld bytes live=%lld bytes\n",
            (long long)zap_gc_count(), (long long)heap, (long long)lb);
    pthread_mutex_lock(&g_all_mu);
    ng = g_all_len;
    fprintf(f, "goroutines: %lld\n", (long long)ng);
    for (int64_t i = 0; i < g_all_len; i++) {
        G *g = g_all[i];
        if (!g) {
            continue;
        }
        const char *own = g->scope ? "scoped" : (g->in_live ? "root" : "detached");
        fprintf(f, "  goroutine %lld [%s, %s]\n", (long long)i,
                gstate_name(g->state), own);
    }
    pthread_mutex_unlock(&g_all_mu);
    fprintf(f, "========================\n");
    fflush(f);
}

void zap_runtime_goroutine_dump(void) { zap_goroutine_dump_to(stderr); }

/* ---- default diagnostic handlers ------------------------------------------ */
static void zap_sig_run_default(int signo) {
    if (signo == SIGUSR1) {
        zap_gc_collect();
        int64_t heap = 0, live = 0;
        zap_gc_stats(&heap, &live);
        fprintf(stderr,
                "=== zap SIGUSR1: forced GC ===\n"
                "heap=%lld bytes  live=%lld bytes  gc-cycles=%lld  goroutines=%lld\n",
                (long long)heap, (long long)live, (long long)zap_gc_count(),
                (long long)zap_runtime_num_goroutines());
        fflush(stderr);
    } else if (signo == SIGQUIT) {
        zap_goroutine_dump_to(stderr);
    }
}

/* ---- dispatch (runs on the dedicated signal thread, normal context) ------- */
static void zap_grace_arm(ScopeSigSub *ss); /* fwd */
static void zap_sig_dispatch(int signo) {
    if (signo <= 0 || signo >= ZAP_NSIG) {
        return;
    }
    pthread_mutex_lock(&g_sig_mu);
    /* Any user handler for this signal — a `notify` subscriber OR a
     * `scope(cancel_on=...)` subscription — takes over its disposition and
     * suppresses the built-in default (D-0058 "a user notify/@on_signal for that
     * signal wins"). */
    int had_sub = (g_sig_subs[signo] != NULL) || (g_scope_subs[signo] != NULL);
    for (SigSub *s = g_sig_subs[signo]; s; s = s->next) {
        chan_offer_word((ZapChan *)s->chan, (uint64_t)(int64_t)signo);
    }
    /* Arm every scope watching this signal; start a grace timer once per scope. */
    for (ScopeSigSub *ss = g_scope_subs[signo]; ss; ss = ss->next) {
        if (ss->completed) {
            continue;
        }
        Scope *sc = ss->scope;
        pthread_mutex_lock(&sc->mu);
        sc->cancelled = 1;
        pthread_cond_broadcast(&sc->cv);
        pthread_mutex_unlock(&sc->mu);
        if (ss->grace_ms > 0) {
            zap_grace_arm(ss);
        }
    }
    int run_default = (!had_sub) && g_sig_default[signo];
    pthread_mutex_unlock(&g_sig_mu);
    /* A cancel_on signal may leave coroutines parked on a channel; they observe
     * the cancel at their next cancellation point (cooperative model). Forcing a
     * wake of an arbitrarily-parked G is the netpoller-backed follow-up. */
    if (run_default) {
        zap_sig_run_default(signo);
    }
}

/* ---- the dedicated signal thread ------------------------------------------ */
/* async-signal-safe: only writes the signo byte to the self-pipe (write(2) is
 * async-signal-safe; a full non-blocking pipe simply coalesces the edge). */
static void zap_sig_handler(int signo) {
    int saved = errno;
    unsigned char b = (unsigned char)signo;
    ssize_t r = write(g_sig_pipe[1], &b, 1);
    (void)r;
    g_sig_delivered++; /* async-signal-safe (sig_atomic_t): test sync counter */
    errno = saved;
}

static void *zap_sig_thread_main(void *arg) {
    (void)arg;
    /* This is the one thread with the handled async signals UNBLOCKED, so a
     * process-directed signal is delivered here (its async-safe handler writes
     * the self-pipe) rather than to a scheduler worker mid context-switch
     * (D-0058 "scheduler threads mask signals"). Start from an empty mask so any
     * signal whose handler is installed later is already unblocked here. */
    sigset_t empty;
    sigemptyset(&empty);
    pthread_sigmask(SIG_SETMASK, &empty, NULL);
    /* Registered mutator (Immix): this thread parks via zap_netpoll_wait ->
     * gopark's native path, which brackets the wait with enter/leave_blocking;
     * it never allocates GC memory and is GC-safe while blocked (like main). */
    zap_mutator_register();
    unsigned char buf[64];
    for (;;) {
        int64_t r = zap_netpoll_wait(g_sig_pd, ZAP_POLL_READ, 0);
        if (g_sig_shutdown) {
            break;
        }
        if (r == ZAP_POLL_CLOSED) {
            break;
        }
        /* Edge-triggered: drain the pipe to EAGAIN, dispatching each signo. */
        for (;;) {
            ssize_t n = read(g_sig_pipe[0], buf, sizeof buf);
            if (n <= 0) {
                break;
            }
            for (ssize_t i = 0; i < n; i++) {
                if (buf[i] == ZAP_SIG_SYNC_BYTE) {
                    /* Drain barrier: every earlier edge in this FIFO has now been
                     * dispatched, so release any waiter in zap_signal_drain. */
                    pthread_mutex_lock(&g_sig_sync_mu);
                    g_sig_sync_seq++;
                    pthread_cond_broadcast(&g_sig_sync_cv);
                    pthread_mutex_unlock(&g_sig_sync_mu);
                } else {
                    zap_sig_dispatch((int)buf[i]);
                }
            }
        }
    }
    return NULL;
}

/* ---- fault (crash) handler ------------------------------------------------ */
static void zap_fault_handler(int signo, siginfo_t *info, void *uctx) {
    (void)uctx;
    char msg[256];
    void *addr = info ? info->si_addr : NULL;
    M *m = cur_m;
    G *g = m ? m->current : NULL;
    int n = snprintf(msg, sizeof msg,
                     "\n=== zap fatal: %s (signal %d) ===\n"
                     "faulting address: %p\n"
                     "offending goroutine: ",
                     sig_name(signo), signo, addr);
    if (n > 0) {
        ssize_t w = write(STDERR_FILENO, msg, (size_t)n);
        (void)w;
    }
    if (g) {
        int m2 = snprintf(msg, sizeof msg, "goroutine %lld [%s]\n",
                          (long long)g->all_idx, gstate_name(g->state));
        if (m2 > 0) {
            ssize_t w = write(STDERR_FILENO, msg, (size_t)m2);
            (void)w;
        }
    } else {
        const char *mt = "main/native thread\n";
        ssize_t w = write(STDERR_FILENO, mt, strlen(mt));
        (void)w;
    }
#ifdef ZAP_HAVE_BACKTRACE
    {
        const char *hdr = "backtrace:\n";
        ssize_t w = write(STDERR_FILENO, hdr, strlen(hdr));
        (void)w;
        void *frames[64];
        int nf = backtrace(frames, 64);
        backtrace_symbols_fd(frames, nf, STDERR_FILENO); /* async-signal-safe */
    }
#endif
    /* Restore the default disposition and re-raise so the process dies with the
     * correct status (and dumps core if enabled). */
    signal(signo, SIG_DFL);
    raise(signo);
}

static char g_sig_altstack[ZAP_ALTSTACK_SIZE];
static void zap_install_fault_handlers(void) {
    stack_t ss;
    ss.ss_sp = g_sig_altstack;
    ss.ss_size = sizeof g_sig_altstack;
    ss.ss_flags = 0;
    sigaltstack(&ss, NULL);
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = zap_fault_handler;
    sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGFPE, &sa, NULL);
    sigaction(SIGBUS, &sa, NULL);
    sigaction(SIGILL, &sa, NULL);
}

/* Install (idempotently) the async self-pipe handler for `signo`. Caller holds
 * g_sig_mu. SA_RESTART so a handler interrupting a slow syscall resumes it. */
static void zap_sig_install(int signo) {
    if (signo <= 0 || signo >= ZAP_NSIG || g_sig_installed[signo]) {
        return;
    }
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = zap_sig_handler;
    sa.sa_flags = SA_RESTART;
    sigemptyset(&sa.sa_mask);
    if (sigaction(signo, &sa, NULL) == 0) {
        g_sig_installed[signo] = 1;
        /* Block this signal on the installing thread so it is delivered to the
         * dedicated signal thread (the only one that unblocks it). Done on the
         * main thread at startup (so scheduler workers, created later, inherit
         * the block); best-effort for a signal first subscribed from a worker. */
        sigset_t s;
        sigemptyset(&s);
        sigaddset(&s, signo);
        pthread_sigmask(SIG_BLOCK, &s, NULL);
    }
}

/* ---- lazy start / shutdown ------------------------------------------------ */
static void zap_sig_start_once(void) {
    if (pipe(g_sig_pipe) != 0) {
        fprintf(stderr, "zap: signal self-pipe creation failed\n");
        return;
    }
    /* Non-blocking + close-on-exec on both ends (the handler's write must never
     * block; the read end is drained edge-triggered). */
    for (int i = 0; i < 2; i++) {
        int fl = fcntl(g_sig_pipe[i], F_GETFL, 0);
        fcntl(g_sig_pipe[i], F_SETFL, fl | O_NONBLOCK);
        int fd = fcntl(g_sig_pipe[i], F_GETFD, 0);
        fcntl(g_sig_pipe[i], F_SETFD, fd | FD_CLOEXEC);
    }
    g_sig_pd = zap_netpoll_open(g_sig_pipe[0]);
    if (!g_sig_pd) {
        /* Netpoller unavailable (non-Linux v1): signals degrade to no-ops. */
        close(g_sig_pipe[0]);
        close(g_sig_pipe[1]);
        g_sig_pipe[0] = g_sig_pipe[1] = -1;
        return;
    }
    zap_install_fault_handlers();
    int rc = GC_pthread_create(&g_sig_thread, NULL, zap_sig_thread_main, NULL);
    if (rc != 0) {
        fprintf(stderr, "zap: signal thread start failed (%d)\n", rc);
        return;
    }
    g_sig_active = 1;
}

static int zap_signal_ensure(void) {
    pthread_once(&g_sig_once, zap_sig_start_once);
    return g_sig_active ? 0 : -1;
}

/* Stop the signal subsystem (program exit, from zap_rt_join_all). Wakes the
 * signal thread off its netpoller park and joins it. No-op if never started. */
static void zap_signal_shutdown(void) {
    if (!g_sig_active) {
        return;
    }
    g_sig_shutdown = 1;
    /* Wake the signal thread off its netpoller park so it observes g_sig_shutdown
     * and exits. zap_netpoll_wake now LATCHES when the thread is momentarily
     * between waits (not parked), so this wake is race-free — a plain self-pipe
     * write would rely on an EPOLLET edge that is lost if the pipe is non-empty,
     * which is exactly how this join used to hang intermittently. */
    if (g_sig_pd) {
        zap_netpoll_wake(g_sig_pd, ZAP_POLL_READ);
    }
    GC_pthread_join(g_sig_thread, NULL);
    if (g_sig_pd) {
        zap_netpoll_close(g_sig_pd);
        g_sig_pd = NULL;
    }
    if (g_sig_pipe[0] >= 0) {
        close(g_sig_pipe[0]);
    }
    if (g_sig_pipe[1] >= 0) {
        close(g_sig_pipe[1]);
    }
    g_sig_active = 0;
}

/* ---- public ABI: program-prologue init (default handlers) ----------------- */
/* Emitted at the top of `main` by the lowerer. Starts the subsystem and installs
 * the built-in default handlers, unless the runtime is built -DZAP_NO_DEFAULT_SIGNALS
 * (opt-out, D-0058). A user `notify`/`@on_signal` for one of these signals later
 * wins (fan-out suppresses the default — see zap_sig_dispatch). */
void zap_signal_init(void) {
#ifndef ZAP_NO_DEFAULT_SIGNALS
    if (zap_signal_ensure() != 0) {
        return;
    }
    pthread_mutex_lock(&g_sig_mu);
    g_sig_default[SIGQUIT] = 1;
    zap_sig_install(SIGQUIT);
    g_sig_default[SIGUSR1] = 1;
    zap_sig_install(SIGUSR1);
    pthread_mutex_unlock(&g_sig_mu);
#endif
}

/* ---- public ABI: signal.notify(signo) -> channel[Signal] ------------------ */
void *zap_signal_notify(int64_t signo) {
    if (zap_signal_ensure() != 0 || signo <= 0 || signo >= ZAP_NSIG) {
        return zap_chan_new(1); /* inert channel: subsystem unavailable */
    }
    void *ch = zap_chan_new(1); /* cap-1 buffered + coalescing (D-0058) */
    SigSub *s = (SigSub *)zap_obj(&zap_desc_sigsub);
    s->chan = ch;
    pthread_mutex_lock(&g_sig_mu);
    s->next = g_sig_subs[signo];
    g_sig_subs[signo] = s;
    zap_sig_install((int)signo); /* notify takes over disposition */
    pthread_mutex_unlock(&g_sig_mu);
    return ch;
}

/* ---- public ABI: @on_signal(signo) handler registration ------------------- */
typedef void (*zap_sig_cb)(void);
typedef struct {
    void *ch;
    zap_sig_cb fn;
} SigCbArg;

/* A detached goroutine that invokes the decorated handler on each delivery. It
 * parks on the notify channel (real G, green stack), so the handler runs in full
 * Zap context — never on the signal thread. */
static void *zap_sig_cb_loop(void *arg) {
    SigCbArg *a = (SigCbArg *)arg;
    for (;;) {
        (void)zap_chan_recv_i64(a->ch); /* block until a signal is delivered */
        a->fn();
    }
    return NULL;
}

void zap_signal_on(int64_t signo, void *fn) {
    void *ch = zap_signal_notify(signo);
    SigCbArg *a = (SigCbArg *)zap_buf((int64_t)sizeof(SigCbArg));
    a->ch = ch;
    a->fn = (zap_sig_cb)fn;
    /* `a` holds `ch` (already rooted by its SigSub) and a code pointer; it is
     * reachable from the spawned G's arg field, which both collectors trace. */
    zap_spawn_detached(zap_sig_cb_loop, a);
}

/* ---- public ABI: scope(cancel_on=mask, grace=ms) -------------------------- */
/* Grace hard-exit timer: a plain (non-GC) detached thread. It references only
 * the malloc'd ScopeSigSub (refcounted), never the GC Scope, so it is immune to
 * the Scope being collected after the block exits. If the scope has not left its
 * block (completed) when the grace period elapses, force process exit. */
static void *zap_grace_timer(void *arg) {
    ScopeSigSub *ss = (ScopeSigSub *)arg;
    int64_t ms = ss->grace_ms;
    struct timespec ts;
    ts.tv_sec = ms / 1000;
    ts.tv_nsec = (ms % 1000) * 1000000L;
    nanosleep(&ts, NULL);
    pthread_mutex_lock(&g_sig_mu);
    int done = ss->completed;
    if (--ss->refcnt == 0) {
        free(ss);
    }
    pthread_mutex_unlock(&g_sig_mu);
    if (!done) {
        fprintf(stderr,
                "zap: graceful-shutdown grace period (%lldms) elapsed; forcing exit\n",
                (long long)ms);
        fflush(stderr);
        _exit(1);
    }
    return NULL;
}

/* Caller holds g_sig_mu. Start one grace timer for this subscription. */
static void zap_grace_arm(ScopeSigSub *ss) {
    ss->refcnt++;
    pthread_t t;
    if (pthread_create(&t, NULL, zap_grace_timer, ss) != 0) {
        ss->refcnt--; /* could not arm: drop the extra ref */
        return;
    }
    pthread_detach(t);
}

/* `scope(cancel_on=[..], grace=ms)` (ticket #77): arm `scope` to cancel when any
 * signal in `mask` (bit N = signal N) fires; `grace_ms > 0` adds a hard-exit
 * deadline measured from the first such signal. Called right after
 * zap_scope_enter; paired with zap_scope_disarm at the scope's dedent. */
void zap_scope_cancel_on(void *scope_, int64_t mask, int64_t grace_ms) {
    if (zap_signal_ensure() != 0 || mask == 0) {
        return;
    }
    Scope *sc = (Scope *)scope_;
    pthread_mutex_lock(&g_sig_mu);
    for (int s = 1; s < ZAP_NSIG && s < 63; s++) {
        if (!(mask & (1LL << s))) {
            continue;
        }
        ScopeSigSub *n = (ScopeSigSub *)malloc(sizeof(ScopeSigSub));
        if (!n) {
            continue;
        }
        n->scope = sc;
        n->grace_ms = grace_ms;
        n->completed = 0;
        n->refcnt = 1;
        n->next = g_scope_subs[s];
        g_scope_subs[s] = n;
        zap_sig_install(s);
    }
    pthread_mutex_unlock(&g_sig_mu);
}

/* Remove every cancel_on subscription for `scope` (its block has exited). Marks
 * each node completed so a pending grace timer stands down, and frees nodes with
 * no outstanding timer. */
void zap_scope_disarm(void *scope_) {
    Scope *sc = (Scope *)scope_;
    pthread_mutex_lock(&g_sig_mu);
    for (int s = 1; s < ZAP_NSIG; s++) {
        ScopeSigSub **pp = &g_scope_subs[s];
        while (*pp) {
            ScopeSigSub *n = *pp;
            if (n->scope == sc) {
                *pp = n->next;
                n->completed = 1;
                if (--n->refcnt == 0) {
                    free(n);
                }
            } else {
                pp = &n->next;
            }
        }
    }
    pthread_mutex_unlock(&g_sig_mu);
}

/* ---- Immix root marker: keep notify channels + sub nodes reachable -------- */
#ifdef ZAP_GC_IMMIX
static void zap_sig_mark_roots(void) {
    pthread_mutex_lock(&g_sig_mu);
    for (int s = 0; s < ZAP_NSIG; s++) {
        /* Marking each head traces the whole {next, chan} chain via the precise
         * SigSub descriptor (the marker is a transitive BFS worklist). */
        zap_gc_mark(g_sig_subs[s]);
    }
    pthread_mutex_unlock(&g_sig_mu);
}
#endif

/* ---- test-only: raise a signal to this process (ticket #77) --------------- *
 * Backs the internal `__raise_signal(n)` builtin used by the signal goldens.
 * PROCESS-directed (kill(getpid)) rather than thread-directed (raise), so that
 * with the handled signals masked on the caller the kernel delivers to the
 * dedicated signal thread — exactly the production path. Test scaffolding, like
 * `__signal_selftest`; it is not a public language surface. */
void zap_signal_raise(int64_t signo) {
    int s = (int)signo;
    int is_fault = (s == SIGSEGV || s == SIGFPE || s == SIGBUS || s == SIGILL);
    if (is_fault || !g_sig_active) {
        kill(getpid(), s); /* fault: the crash handler runs + re-raises (no return) */
        return;
    }
    sig_atomic_t before = g_sig_delivered;
    kill(getpid(), s);
    /* Spin until the async handler has written this edge to the self-pipe, so a
     * following __signal_drain sees it first in FIFO order. Bounded so a dropped
     * signal cannot hang a test. */
    for (int i = 0; i < 2000000 && g_sig_delivered == before; i++) {
        sched_yield();
    }
}

/* Block until the signal thread has dispatched every edge raised so far
 * (ticket #77 test sync; backs `__signal_drain`). Pushes a sentinel behind the
 * pending edges and waits for the signal thread to reach it. */
void zap_signal_drain(void) {
    if (!g_sig_active) {
        return;
    }
    /* Precise GC (#48): this caller (typically a mutator — e.g. the main thread)
     * blocks on the condvar below, so mark it GC-safe. Otherwise a SIGUSR1
     * default handler's forced GC (stop-the-world, run on the signal thread)
     * would deadlock waiting for this thread to reach a safepoint it can never
     * reach while parked here. Its roots were flushed on entry (same contract as
     * gopark's native-waiter path / scope_join). */
    zap_enter_blocking();
    pthread_mutex_lock(&g_sig_sync_mu);
    int64_t want = g_sig_sync_seq + 1;
    unsigned char b = ZAP_SIG_SYNC_BYTE;
    ssize_t r;
    do {
        r = write(g_sig_pipe[1], &b, 1);
    } while (r < 0 && errno == EINTR);
    while (g_sig_sync_seq < want) {
        pthread_cond_wait(&g_sig_sync_cv, &g_sig_sync_mu);
    }
    pthread_mutex_unlock(&g_sig_sync_mu);
    zap_leave_blocking();
}

/* ---- C-level integration self-test (ticket #77) --------------------------- *
 * No language I/O yet at bring-up, so this drives the self-pipe + netpoller +
 * dispatch end-to-end from C (mirrors __netpoll_selftest). It subscribes to
 * SIGUSR2, raises it twice in quick succession (the second must COALESCE on the
 * cap-1 channel), receives exactly one delivery of the right value, confirms the
 * second was dropped, and returns the number of sub-checks that passed
 * (ZAP_SIG_SELFTEST_CHECKS on full success). */
#define ZAP_SIG_SELFTEST_CHECKS 2
int64_t zap_signal_selftest(void) {
    int64_t passed = 0;
    void *ch = zap_signal_notify(SIGUSR2);
    if (!g_sig_active) {
        fprintf(stderr, "signal selftest: subsystem inactive\n");
        return 0;
    }
    /* Deliver two edges; coalescing means the cap-1 channel holds one. Must be
     * PROCESS-directed (kill(getpid)), not thread-directed (raise): the handled
     * signal is masked on this thread and unblocked only on the signal thread. */
    kill(getpid(), SIGUSR2);
    /* Spin briefly so the signal thread delivers the first edge before the
     * second arrives (keeps the test deterministic without a second channel). */
    for (int i = 0; i < 100; i++) {
        struct timespec ts = {0, 1000000L}; /* 1 ms */
        nanosleep(&ts, NULL);
        pthread_mutex_lock(&((ZapChan *)ch)->mu);
        int64_t cnt = ((ZapChan *)ch)->count;
        pthread_mutex_unlock(&((ZapChan *)ch)->mu);
        if (cnt > 0) {
            break;
        }
    }
    kill(getpid(), SIGUSR2); /* second edge: buffer full -> coalesced (dropped) */
    struct timespec ts = {0, 20000000L}; /* 20 ms settle */
    nanosleep(&ts, NULL);
    int64_t got = zap_chan_recv_i64(ch); /* one delivery */
    if (got == SIGUSR2) {
        passed++;
    } else {
        fprintf(stderr, "signal selftest: delivered %lld, want %d\n",
                (long long)got, SIGUSR2);
    }
    /* The coalesced edge must have left the channel empty (no second value). */
    pthread_mutex_lock(&((ZapChan *)ch)->mu);
    int64_t remaining = ((ZapChan *)ch)->count;
    pthread_mutex_unlock(&((ZapChan *)ch)->mu);
    if (remaining == 0) {
        passed++;
    } else {
        fprintf(stderr, "signal selftest: %lld edges not coalesced\n",
                (long long)remaining);
    }
    return passed;
}

/* ===========================================================================
 * os / sys / io / fs / process (ticket #97, D-0056). The system, I/O and
 * process core of the bundled stdlib, dispatched by flat name through the
 * module seam (#121): `os`/`sys`/`io`/`fs`/`process` are compiler-known
 * namespaced modules (`import` accepted but not required), exactly like
 * `signal`/`gc`/`runtime`.
 *
 * Design (flagged in the PR):
 *   - Files are raw int64 fds (no handle object / prelude type); the stdio
 *     handles `sys.std{in,out,err}_fd()` are the constants 0/1/2 (lowered
 *     inline, no runtime call here).
 *   - `fs.stat`-style queries are separate scalar/bool entry points
 *     (`fs.size/is_dir/is_file/exists`) so no stat struct is needed.
 *   - `process.run(list[str])` captures stdout/stderr into temp files (NOT
 *     pipes) and `waitpid`s, so there is no pipe-buffer deadlock and no fork;
 *     it returns a GC `ZapProc` whose three fields the lowerer reads back into
 *     a `(int64, str, str)` tuple = (exit_code, stdout, stderr).
 *   - Fallible calls raise a catchable Zap exception via `zap_panic` (the
 *     message is "<op>: <errno text>"), matching the rest of the runtime.
 *
 * GC safety: a collection only ever runs at a compiled-code safepoint (see
 * zapgc.inc.c), never inside a runtime C call, so GC objects built here and
 * held in C locals stay live for the duration of the call without rooting.
 * The one blocking point (`waitpid`) is wrapped in zap_enter/leave_blocking so
 * a concurrent stop-the-world GC need not wait for this thread (its roots are
 * already flushed; no un-flushed Zap temporaries are live across the wait).
 * ========================================================================== */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <spawn.h>
#include <stddef.h> /* offsetof */
#include <sys/stat.h>
#include <sys/wait.h>
#if defined(__APPLE__)
#include <crt_externs.h> /* _NSGetArgc / _NSGetArgv */
#endif

extern char **environ;

/* NUL-terminated copy of a ZapStr for the C-string syscall APIs (ZapStr data is
 * length-prefixed, not NUL-terminated). The buffer is a GC leaf; no safepoint
 * runs inside a runtime call, so holding it in a C local is safe. */
static char *zap_sys_cstr(const ZapStr *s) {
    char *buf = (char *)zap_buf((int64_t)s->len + 1);
    if (s->len > 0) {
        memcpy(buf, s->data, (size_t)s->len);
    }
    buf[s->len] = '\0';
    return buf;
}

/* Raise a catchable Zap exception "<op>: <detail>" (never returns). */
static void zap_sys_fail(const char *op, const char *detail) {
    char buf[512];
    int n = snprintf(buf, sizeof buf, "%s: %s", op, detail);
    if (n < 0) {
        n = 0;
    }
    if (n >= (int)sizeof buf) {
        n = (int)sizeof buf - 1;
    }
    zap_panic(zap_str_new(buf, (int64_t)n));
}

/* --- os: environment -------------------------------------------------------- */

/* os.getenv(name) -> str: the variable's value, or "" when unset (use
 * os.hasenv to distinguish unset from empty; there is no None in v1). */
void *zap_os_getenv(void *name_) {
    const char *v = getenv(zap_sys_cstr((ZapStr *)name_));
    return v ? zap_str_new(v, (int64_t)strlen(v)) : zap_str_new("", 0);
}

/* os.hasenv(name) -> bool. */
int64_t zap_os_hasenv(void *name_) {
    return getenv(zap_sys_cstr((ZapStr *)name_)) != NULL ? 1 : 0;
}

/* os.setenv(name, value) (overwrites). */
void zap_os_setenv(void *name_, void *val_) {
    if (setenv(zap_sys_cstr((ZapStr *)name_), zap_sys_cstr((ZapStr *)val_), 1) != 0) {
        zap_sys_fail("os.setenv", strerror(errno));
    }
}

/* os.getpid() -> int64. */
int64_t zap_os_getpid(void) { return (int64_t)getpid(); }

/* --- os: working directory & entries ---------------------------------------- */

/* os.getcwd() -> str. */
void *zap_os_getcwd(void) {
    char buf[4096];
    if (getcwd(buf, sizeof buf) == NULL) {
        zap_sys_fail("os.getcwd", strerror(errno));
    }
    return zap_str_new(buf, (int64_t)strlen(buf));
}

/* os.chdir(path). */
void zap_os_chdir(void *path_) {
    if (chdir(zap_sys_cstr((ZapStr *)path_)) != 0) {
        zap_sys_fail("os.chdir", strerror(errno));
    }
}

/* os.mkdir(path) (mode 0777 before umask). */
void zap_os_mkdir(void *path_) {
    if (mkdir(zap_sys_cstr((ZapStr *)path_), 0777) != 0) {
        zap_sys_fail("os.mkdir", strerror(errno));
    }
}

/* os.remove(path): unlink a file. */
void zap_os_remove(void *path_) {
    if (unlink(zap_sys_cstr((ZapStr *)path_)) != 0) {
        zap_sys_fail("os.remove", strerror(errno));
    }
}

/* os.rmdir(path): remove an empty directory. */
void zap_os_rmdir(void *path_) {
    if (rmdir(zap_sys_cstr((ZapStr *)path_)) != 0) {
        zap_sys_fail("os.rmdir", strerror(errno));
    }
}

/* os.listdir(path) / fs.listdir(path) -> list[str]: entry names (excluding
 * "." and ".."), in unspecified order. */
void *zap_os_listdir(void *path_) {
    char *path = zap_sys_cstr((ZapStr *)path_);
    DIR *d = opendir(path);
    if (d == NULL) {
        zap_sys_fail("os.listdir", strerror(errno));
    }
    void *out = zap_list_new();
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0) {
            continue;
        }
        zap_list_push_ptr(out, zap_str_new(e->d_name, (int64_t)strlen(e->d_name)));
    }
    closedir(d);
    return out;
}

/* --- os.path helpers (flat names: Zap dispatch is `module.method`) ---------- */

/* os.path_join(a, b): join with a single '/'. An absolute `b` replaces `a`
 * (POSIX semantics). */
void *zap_os_path_join(void *a_, void *b_) {
    ZapStr *a = (ZapStr *)a_;
    ZapStr *b = (ZapStr *)b_;
    if (b->len > 0 && b->data[0] == '/') {
        return zap_str_new(b->data, b->len);
    }
    if (a->len == 0) {
        return zap_str_new(b->data, b->len);
    }
    int need_sep = a->data[a->len - 1] != '/';
    int64_t n = a->len + (need_sep ? 1 : 0) + b->len;
    char *buf = (char *)zap_buf(n);
    memcpy(buf, a->data, (size_t)a->len);
    int64_t off = a->len;
    if (need_sep) {
        buf[off++] = '/';
    }
    memcpy(buf + off, b->data, (size_t)b->len);
    return zap_str_new(buf, n);
}

/* os.path_exists(path) -> bool. */
int64_t zap_os_path_exists(void *path_) {
    return access(zap_sys_cstr((ZapStr *)path_), F_OK) == 0 ? 1 : 0;
}

/* os.path_basename(path): the final component ("" for a trailing-slash path). */
void *zap_os_path_basename(void *path_) {
    ZapStr *p = (ZapStr *)path_;
    int64_t i = p->len;
    while (i > 0 && p->data[i - 1] == '/') {
        i--; /* ignore trailing slashes, like POSIX basename of "a/" -> "a" */
    }
    int64_t end = i;
    while (i > 0 && p->data[i - 1] != '/') {
        i--;
    }
    return zap_str_new(p->data + i, end - i);
}

/* os.path_dirname(path): everything up to the last '/' (no trailing slash);
 * "." when there is no '/'. */
void *zap_os_path_dirname(void *path_) {
    ZapStr *p = (ZapStr *)path_;
    int64_t i = p->len;
    while (i > 0 && p->data[i - 1] == '/') {
        i--;
    }
    while (i > 0 && p->data[i - 1] != '/') {
        i--;
    }
    while (i > 1 && p->data[i - 1] == '/') {
        i--; /* collapse the trailing separator(s), keeping a lone leading "/" */
    }
    if (i == 0) {
        return zap_str_new(".", 1);
    }
    return zap_str_new(p->data, i);
}

/* --- sys -------------------------------------------------------------------- */

#if defined(__APPLE__)
static int zap_prog_argc(void) { return *_NSGetArgc(); }
static char **zap_prog_argv(void) { return *_NSGetArgv(); }
#else
/* On ELF platforms the dynamic loader passes (argc, argv, envp) to init-array
 * constructors; capture them for sys.argv (there is no argv parameter on the
 * compiled `main`, which has the C signature `int main(void)`). */
static int zap_saved_argc = 0;
static char **zap_saved_argv = NULL;
__attribute__((constructor)) static void zap_capture_argv(int argc, char **argv,
                                                          char **envp) {
    (void)envp;
    zap_saved_argc = argc;
    zap_saved_argv = argv;
}
static int zap_prog_argc(void) { return zap_saved_argc; }
static char **zap_prog_argv(void) { return zap_saved_argv; }
#endif

/* sys.argv() -> list[str]: the process arguments (argv[0] is the program). */
void *zap_sys_argv(void) {
    int argc = zap_prog_argc();
    char **argv = zap_prog_argv();
    void *out = zap_list_new();
    for (int i = 0; i < argc && argv != NULL && argv[i] != NULL; i++) {
        zap_list_push_ptr(out, zap_str_new(argv[i], (int64_t)strlen(argv[i])));
    }
    return out;
}

/* sys.exit(code): terminate the process immediately with `code`. */
void zap_sys_exit(int64_t code) { exit((int)code); }

/* sys.platform() -> str. */
void *zap_sys_platform(void) {
#if defined(__APPLE__)
    return zap_str_new("darwin", 6);
#elif defined(__linux__)
    return zap_str_new("linux", 5);
#else
    return zap_str_new("unknown", 7);
#endif
}

/* --- io: fd-based file I/O -------------------------------------------------- */

/* io.open(path, mode) -> fd. Modes mirror C stdio: r r+ w w+ a a+. */
int64_t zap_io_open(void *path_, void *mode_) {
    const char *mode = zap_sys_cstr((ZapStr *)mode_);
    int flags;
    if (strcmp(mode, "r") == 0) {
        flags = O_RDONLY;
    } else if (strcmp(mode, "r+") == 0) {
        flags = O_RDWR;
    } else if (strcmp(mode, "w") == 0) {
        flags = O_WRONLY | O_CREAT | O_TRUNC;
    } else if (strcmp(mode, "w+") == 0) {
        flags = O_RDWR | O_CREAT | O_TRUNC;
    } else if (strcmp(mode, "a") == 0) {
        flags = O_WRONLY | O_CREAT | O_APPEND;
    } else if (strcmp(mode, "a+") == 0) {
        flags = O_RDWR | O_CREAT | O_APPEND;
    } else {
        zap_sys_fail("io.open", "invalid mode (want one of: r r+ w w+ a a+)");
        return -1; /* unreachable */
    }
    int fd = open(zap_sys_cstr((ZapStr *)path_), flags, 0644);
    if (fd < 0) {
        zap_sys_fail("io.open", strerror(errno));
    }
    return (int64_t)fd;
}

/* io.read(fd, n) -> str: up to `n` bytes ("" at EOF). */
void *zap_io_read(int64_t fd, int64_t n) {
    if (n < 0) {
        n = 0;
    }
    char *buf = (char *)zap_buf(n > 0 ? n : 1);
    ssize_t got;
    do {
        got = read((int)fd, buf, (size_t)n);
    } while (got < 0 && errno == EINTR);
    if (got < 0) {
        zap_sys_fail("io.read", strerror(errno));
    }
    return zap_str_new(buf, (int64_t)got);
}

/* io.write(fd, data) -> int64: bytes written (the full string). */
int64_t zap_io_write(int64_t fd, void *data_) {
    ZapStr *s = (ZapStr *)data_;
    size_t off = 0;
    while (off < (size_t)s->len) {
        ssize_t w = write((int)fd, s->data + off, (size_t)s->len - off);
        if (w < 0) {
            if (errno == EINTR) {
                continue;
            }
            zap_sys_fail("io.write", strerror(errno));
        }
        off += (size_t)w;
    }
    return (int64_t)off;
}

/* io.close(fd). */
void zap_io_close(int64_t fd) {
    if (close((int)fd) != 0) {
        zap_sys_fail("io.close", strerror(errno));
    }
}

/* --- fs: path-oriented convenience + metadata ------------------------------- */

/* Read an entire open fd to a fresh ZapStr (for regular files; uses the file
 * size when seekable, otherwise grows). Caller handles open/close. */
static void *zap_read_all_fd(int fd) {
    off_t end = lseek(fd, 0, SEEK_END);
    int64_t cap = (end > 0) ? (int64_t)end : 4096;
    if (end >= 0) {
        lseek(fd, 0, SEEK_SET);
    }
    char *buf = (char *)zap_buf(cap);
    int64_t got = 0;
    for (;;) {
        if (got == cap) {
            int64_t ncap = cap * 2;
            char *nb = (char *)zap_buf(ncap);
            memcpy(nb, buf, (size_t)got);
            buf = nb;
            cap = ncap;
        }
        ssize_t r = read(fd, buf + got, (size_t)(cap - got));
        if (r < 0) {
            if (errno == EINTR) {
                continue;
            }
            zap_sys_fail("fs.read_file", strerror(errno));
        }
        if (r == 0) {
            break;
        }
        got += (int64_t)r;
    }
    return zap_str_new(buf, got);
}

/* fs.read_file(path) -> str: the whole file. */
void *zap_fs_read_file(void *path_) {
    int fd = open(zap_sys_cstr((ZapStr *)path_), O_RDONLY);
    if (fd < 0) {
        zap_sys_fail("fs.read_file", strerror(errno));
    }
    void *s = zap_read_all_fd(fd);
    close(fd);
    return s;
}

/* fs.write_file(path, data): create/truncate and write the whole string. */
void zap_fs_write_file(void *path_, void *data_) {
    int fd = open(zap_sys_cstr((ZapStr *)path_), O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        zap_sys_fail("fs.write_file", strerror(errno));
    }
    (void)zap_io_write((int64_t)fd, data_);
    close(fd);
}

/* fs.exists(path) -> bool. */
int64_t zap_fs_exists(void *path_) {
    struct stat st;
    return stat(zap_sys_cstr((ZapStr *)path_), &st) == 0 ? 1 : 0;
}

/* fs.is_dir(path) -> bool. */
int64_t zap_fs_is_dir(void *path_) {
    struct stat st;
    if (stat(zap_sys_cstr((ZapStr *)path_), &st) != 0) {
        return 0;
    }
    return S_ISDIR(st.st_mode) ? 1 : 0;
}

/* fs.is_file(path) -> bool. */
int64_t zap_fs_is_file(void *path_) {
    struct stat st;
    if (stat(zap_sys_cstr((ZapStr *)path_), &st) != 0) {
        return 0;
    }
    return S_ISREG(st.st_mode) ? 1 : 0;
}

/* fs.size(path) -> int64: file size in bytes (raises if it does not exist). */
int64_t zap_fs_size(void *path_) {
    struct stat st;
    if (stat(zap_sys_cstr((ZapStr *)path_), &st) != 0) {
        zap_sys_fail("fs.size", strerror(errno));
    }
    return (int64_t)st.st_size;
}

/* fs.mkdtemp() -> str: create a unique temp directory (under $TMPDIR or /tmp)
 * and return its path. Powers hermetic temp-dir workflows (and the goldens). */
void *zap_fs_mkdtemp(void) {
    const char *tmp = getenv("TMPDIR");
    if (tmp == NULL || *tmp == '\0') {
        tmp = "/tmp";
    }
    char tmpl[4096];
    snprintf(tmpl, sizeof tmpl, "%s/zapXXXXXX", tmp);
    if (mkdtemp(tmpl) == NULL) {
        zap_sys_fail("fs.mkdtemp", strerror(errno));
    }
    return zap_str_new(tmpl, (int64_t)strlen(tmpl));
}

/* --- process: subprocess spawn + capture ------------------------------------ */

/* The result of process.run: exit code + captured stdout/stderr. A precise GC
 * object (two trailing pointer fields) so the two ZapStr captures stay live;
 * the lowerer reads the three fields back into a (int64, str, str) tuple. */
typedef struct {
    int64_t code;
    ZapStr *out;
    ZapStr *err;
} ZapProc;

static const int32_t zap_off_proc[] = {
    (int32_t)offsetof(ZapProc, out),
    (int32_t)offsetof(ZapProc, err),
};
static const ZapTypeDesc zap_desc_proc = {(int64_t)sizeof(ZapProc), ZAP_TD_PRECISE,
                                          2, zap_off_proc};

/* process.run(argv: list[str]) -> ZapProc*: spawn argv[0] (PATH-searched) with
 * the given arguments, capturing stdout/stderr into temp files (so there is no
 * pipe-buffer deadlock), and wait. The exit code is the child's status, or
 * 128+signo if it was killed by a signal. Raises on spawn failure. */
void *zap_process_run(void *argv_list) {
    int64_t n = zap_list_len(argv_list);
    if (n < 1) {
        zap_sys_fail("process.run", "empty argument list");
    }
    char **cargv = (char **)malloc(sizeof(char *) * (size_t)(n + 1));
    if (cargv == NULL) {
        zap_sys_fail("process.run", "out of memory");
    }
    for (int64_t i = 0; i < n; i++) {
        cargv[i] = zap_sys_cstr((ZapStr *)zap_list_get_ptr(argv_list, i));
    }
    cargv[n] = NULL;

    const char *tmp = getenv("TMPDIR");
    if (tmp == NULL || *tmp == '\0') {
        tmp = "/tmp";
    }
    char otmpl[4096], etmpl[4096];
    snprintf(otmpl, sizeof otmpl, "%s/zapout_XXXXXX", tmp);
    snprintf(etmpl, sizeof etmpl, "%s/zaperr_XXXXXX", tmp);
    int ofd = mkstemp(otmpl);
    if (ofd < 0) {
        free(cargv);
        zap_sys_fail("process.run", strerror(errno));
    }
    int efd = mkstemp(etmpl);
    if (efd < 0) {
        int e = errno;
        close(ofd);
        unlink(otmpl);
        free(cargv);
        zap_sys_fail("process.run", strerror(e));
    }

    posix_spawn_file_actions_t fa;
    posix_spawn_file_actions_init(&fa);
    posix_spawn_file_actions_adddup2(&fa, ofd, 1);
    posix_spawn_file_actions_adddup2(&fa, efd, 2);
    pid_t pid = 0;
    int rc = posix_spawnp(&pid, cargv[0], &fa, NULL, cargv, environ);
    posix_spawn_file_actions_destroy(&fa);
    free(cargv);
    if (rc != 0) {
        close(ofd);
        close(efd);
        unlink(otmpl);
        unlink(etmpl);
        zap_sys_fail("process.run", strerror(rc));
    }

    int status = 0;
    pid_t w;
    zap_enter_blocking(); /* GC-safe: no un-flushed Zap temporaries across wait */
    do {
        w = waitpid(pid, &status, 0);
    } while (w < 0 && errno == EINTR);
    zap_leave_blocking();

    int64_t code;
    if (w < 0) {
        code = -1;
    } else if (WIFEXITED(status)) {
        code = (int64_t)WEXITSTATUS(status);
    } else if (WIFSIGNALED(status)) {
        code = 128 + (int64_t)WTERMSIG(status);
    } else {
        code = -1;
    }

    ZapProc *r = (ZapProc *)zap_obj(&zap_desc_proc);
    r->code = code;
    r->out = NULL;
    r->err = NULL;
    r->out = (ZapStr *)zap_read_all_fd(ofd);
    r->err = (ZapStr *)zap_read_all_fd(efd);
    close(ofd);
    close(efd);
    unlink(otmpl);
    unlink(etmpl);
    return r;
}

/* Field getters the lowerer calls to unpack the result into a tuple. */
int64_t zap_process_result_code(void *r) { return ((ZapProc *)r)->code; }
void *zap_process_result_stdout(void *r) { return ((ZapProc *)r)->out; }
void *zap_process_result_stderr(void *r) { return ((ZapProc *)r)->err; }

/* ===========================================================================
 * crypto module (ticket #105, D-0056): hashing, HMAC, secure random / secrets.
 * TLS is deferred (late-v1). Self-contained reference implementations of the
 * hashes (FIPS 180-4 SHA-256/SHA-512, FIPS 180-1 SHA-1, RFC 1321 MD5) so no new
 * external dependency is pulled in; HMAC is RFC 2104; the CSPRNG draws from the
 * OS entropy source (getentropy, falling back to /dev/urandom).
 *
 * str/bytes cross as ZapStr* (len-prefixed, binary-safe). Hex digests and raw
 * digests are returned as freshly built ZapStr objects (zap_str_new / a hex
 * helper). A streaming handle is a `bytes` value whose ZapStr `data` buffer
 * holds the hash context; hexdigest/digest finalize a COPY, so a handle may be
 * digested more than once and keeps accepting updates afterwards.
 * ========================================================================== */
#include <fcntl.h>      /* open, O_RDONLY (CSPRNG fallback) */
#include <sys/random.h> /* getentropy */

#define ZAP_CRY_SHA256 0
#define ZAP_CRY_SHA512 1
#define ZAP_CRY_SHA1 2
#define ZAP_CRY_MD5 3

#define ZAP_ROR32(x, n) (((x) >> (n)) | ((x) << (32 - (n))))
#define ZAP_ROL32(x, n) (((x) << (n)) | ((x) >> (32 - (n))))
#define ZAP_ROR64(x, n) (((x) >> (n)) | ((x) << (64 - (n))))

/* ---- SHA-256 (FIPS 180-4) ---- */
typedef struct {
    uint32_t h[8];
    uint64_t len; /* total message bytes */
    uint8_t buf[64];
    size_t n; /* bytes buffered */
} zap_sha256;

static const uint32_t ZAP_SHA256_K[64] = {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1,
    0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
    0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2};

static void zap_sha256_block(zap_sha256 *c, const uint8_t *p) {
    uint32_t w[64], a, b, cc, d, e, f, g, h, t1, t2;
    for (int i = 0; i < 16; i++) {
        w[i] = ((uint32_t)p[i * 4] << 24) | ((uint32_t)p[i * 4 + 1] << 16) |
               ((uint32_t)p[i * 4 + 2] << 8) | (uint32_t)p[i * 4 + 3];
    }
    for (int i = 16; i < 64; i++) {
        uint32_t s0 = ZAP_ROR32(w[i - 15], 7) ^ ZAP_ROR32(w[i - 15], 18) ^
                      (w[i - 15] >> 3);
        uint32_t s1 =
            ZAP_ROR32(w[i - 2], 17) ^ ZAP_ROR32(w[i - 2], 19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    a = c->h[0];
    b = c->h[1];
    cc = c->h[2];
    d = c->h[3];
    e = c->h[4];
    f = c->h[5];
    g = c->h[6];
    h = c->h[7];
    for (int i = 0; i < 64; i++) {
        uint32_t S1 = ZAP_ROR32(e, 6) ^ ZAP_ROR32(e, 11) ^ ZAP_ROR32(e, 25);
        uint32_t ch = (e & f) ^ (~e & g);
        t1 = h + S1 + ch + ZAP_SHA256_K[i] + w[i];
        uint32_t S0 = ZAP_ROR32(a, 2) ^ ZAP_ROR32(a, 13) ^ ZAP_ROR32(a, 22);
        uint32_t maj = (a & b) ^ (a & cc) ^ (b & cc);
        t2 = S0 + maj;
        h = g;
        g = f;
        f = e;
        e = d + t1;
        d = cc;
        cc = b;
        b = a;
        a = t1 + t2;
    }
    c->h[0] += a;
    c->h[1] += b;
    c->h[2] += cc;
    c->h[3] += d;
    c->h[4] += e;
    c->h[5] += f;
    c->h[6] += g;
    c->h[7] += h;
}

static void zap_sha256_init(zap_sha256 *c) {
    c->h[0] = 0x6a09e667;
    c->h[1] = 0xbb67ae85;
    c->h[2] = 0x3c6ef372;
    c->h[3] = 0xa54ff53a;
    c->h[4] = 0x510e527f;
    c->h[5] = 0x9b05688c;
    c->h[6] = 0x1f83d9ab;
    c->h[7] = 0x5be0cd19;
    c->len = 0;
    c->n = 0;
}

static void zap_sha256_update(zap_sha256 *c, const uint8_t *d, size_t n) {
    c->len += n;
    for (size_t i = 0; i < n; i++) {
        c->buf[c->n++] = d[i];
        if (c->n == 64) {
            zap_sha256_block(c, c->buf);
            c->n = 0;
        }
    }
}

static void zap_sha256_final(zap_sha256 *c, uint8_t *out) {
    uint64_t bits = c->len * 8;
    size_t i = c->n;
    c->buf[i++] = 0x80;
    if (i > 56) {
        while (i < 64)
            c->buf[i++] = 0;
        zap_sha256_block(c, c->buf);
        i = 0;
    }
    while (i < 56)
        c->buf[i++] = 0;
    for (int j = 7; j >= 0; j--)
        c->buf[i++] = (uint8_t)(bits >> (j * 8));
    zap_sha256_block(c, c->buf);
    for (int k = 0; k < 8; k++) {
        out[k * 4] = (uint8_t)(c->h[k] >> 24);
        out[k * 4 + 1] = (uint8_t)(c->h[k] >> 16);
        out[k * 4 + 2] = (uint8_t)(c->h[k] >> 8);
        out[k * 4 + 3] = (uint8_t)c->h[k];
    }
}

/* ---- SHA-512 (FIPS 180-4) ---- */
typedef struct {
    uint64_t h[8];
    uint64_t len; /* total message bytes (< 2^61, so a 64-bit bit count fits) */
    uint8_t buf[128];
    size_t n;
} zap_sha512;

static const uint64_t ZAP_SHA512_K[80] = {
    0x428a2f98d728ae22ULL, 0x7137449123ef65cdULL, 0xb5c0fbcfec4d3b2fULL,
    0xe9b5dba58189dbbcULL, 0x3956c25bf348b538ULL, 0x59f111f1b605d019ULL,
    0x923f82a4af194f9bULL, 0xab1c5ed5da6d8118ULL, 0xd807aa98a3030242ULL,
    0x12835b0145706fbeULL, 0x243185be4ee4b28cULL, 0x550c7dc3d5ffb4e2ULL,
    0x72be5d74f27b896fULL, 0x80deb1fe3b1696b1ULL, 0x9bdc06a725c71235ULL,
    0xc19bf174cf692694ULL, 0xe49b69c19ef14ad2ULL, 0xefbe4786384f25e3ULL,
    0x0fc19dc68b8cd5b5ULL, 0x240ca1cc77ac9c65ULL, 0x2de92c6f592b0275ULL,
    0x4a7484aa6ea6e483ULL, 0x5cb0a9dcbd41fbd4ULL, 0x76f988da831153b5ULL,
    0x983e5152ee66dfabULL, 0xa831c66d2db43210ULL, 0xb00327c898fb213fULL,
    0xbf597fc7beef0ee4ULL, 0xc6e00bf33da88fc2ULL, 0xd5a79147930aa725ULL,
    0x06ca6351e003826fULL, 0x142929670a0e6e70ULL, 0x27b70a8546d22ffcULL,
    0x2e1b21385c26c926ULL, 0x4d2c6dfc5ac42aedULL, 0x53380d139d95b3dfULL,
    0x650a73548baf63deULL, 0x766a0abb3c77b2a8ULL, 0x81c2c92e47edaee6ULL,
    0x92722c851482353bULL, 0xa2bfe8a14cf10364ULL, 0xa81a664bbc423001ULL,
    0xc24b8b70d0f89791ULL, 0xc76c51a30654be30ULL, 0xd192e819d6ef5218ULL,
    0xd69906245565a910ULL, 0xf40e35855771202aULL, 0x106aa07032bbd1b8ULL,
    0x19a4c116b8d2d0c8ULL, 0x1e376c085141ab53ULL, 0x2748774cdf8eeb99ULL,
    0x34b0bcb5e19b48a8ULL, 0x391c0cb3c5c95a63ULL, 0x4ed8aa4ae3418acbULL,
    0x5b9cca4f7763e373ULL, 0x682e6ff3d6b2b8a3ULL, 0x748f82ee5defb2fcULL,
    0x78a5636f43172f60ULL, 0x84c87814a1f0ab72ULL, 0x8cc702081a6439ecULL,
    0x90befffa23631e28ULL, 0xa4506cebde82bde9ULL, 0xbef9a3f7b2c67915ULL,
    0xc67178f2e372532bULL, 0xca273eceea26619cULL, 0xd186b8c721c0c207ULL,
    0xeada7dd6cde0eb1eULL, 0xf57d4f7fee6ed178ULL, 0x06f067aa72176fbaULL,
    0x0a637dc5a2c898a6ULL, 0x113f9804bef90daeULL, 0x1b710b35131c471bULL,
    0x28db77f523047d84ULL, 0x32caab7b40c72493ULL, 0x3c9ebe0a15c9bebcULL,
    0x431d67c49c100d4cULL, 0x4cc5d4becb3e42b6ULL, 0x597f299cfc657e2aULL,
    0x5fcb6fab3ad6faecULL, 0x6c44198c4a475817ULL};

static void zap_sha512_block(zap_sha512 *c, const uint8_t *p) {
    uint64_t w[80], a, b, cc, d, e, f, g, h, t1, t2;
    for (int i = 0; i < 16; i++) {
        w[i] = 0;
        for (int j = 0; j < 8; j++)
            w[i] = (w[i] << 8) | (uint64_t)p[i * 8 + j];
    }
    for (int i = 16; i < 80; i++) {
        uint64_t s0 = ZAP_ROR64(w[i - 15], 1) ^ ZAP_ROR64(w[i - 15], 8) ^
                      (w[i - 15] >> 7);
        uint64_t s1 =
            ZAP_ROR64(w[i - 2], 19) ^ ZAP_ROR64(w[i - 2], 61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    a = c->h[0];
    b = c->h[1];
    cc = c->h[2];
    d = c->h[3];
    e = c->h[4];
    f = c->h[5];
    g = c->h[6];
    h = c->h[7];
    for (int i = 0; i < 80; i++) {
        uint64_t S1 = ZAP_ROR64(e, 14) ^ ZAP_ROR64(e, 18) ^ ZAP_ROR64(e, 41);
        uint64_t ch = (e & f) ^ (~e & g);
        t1 = h + S1 + ch + ZAP_SHA512_K[i] + w[i];
        uint64_t S0 = ZAP_ROR64(a, 28) ^ ZAP_ROR64(a, 34) ^ ZAP_ROR64(a, 39);
        uint64_t maj = (a & b) ^ (a & cc) ^ (b & cc);
        t2 = S0 + maj;
        h = g;
        g = f;
        f = e;
        e = d + t1;
        d = cc;
        cc = b;
        b = a;
        a = t1 + t2;
    }
    c->h[0] += a;
    c->h[1] += b;
    c->h[2] += cc;
    c->h[3] += d;
    c->h[4] += e;
    c->h[5] += f;
    c->h[6] += g;
    c->h[7] += h;
}

static void zap_sha512_init(zap_sha512 *c) {
    c->h[0] = 0x6a09e667f3bcc908ULL;
    c->h[1] = 0xbb67ae8584caa73bULL;
    c->h[2] = 0x3c6ef372fe94f82bULL;
    c->h[3] = 0xa54ff53a5f1d36f1ULL;
    c->h[4] = 0x510e527fade682d1ULL;
    c->h[5] = 0x9b05688c2b3e6c1fULL;
    c->h[6] = 0x1f83d9abfb41bd6bULL;
    c->h[7] = 0x5be0cd19137e2179ULL;
    c->len = 0;
    c->n = 0;
}

static void zap_sha512_update(zap_sha512 *c, const uint8_t *d, size_t n) {
    c->len += n;
    for (size_t i = 0; i < n; i++) {
        c->buf[c->n++] = d[i];
        if (c->n == 128) {
            zap_sha512_block(c, c->buf);
            c->n = 0;
        }
    }
}

static void zap_sha512_final(zap_sha512 *c, uint8_t *out) {
    uint64_t bits = c->len * 8;
    size_t i = c->n;
    c->buf[i++] = 0x80;
    if (i > 112) {
        while (i < 128)
            c->buf[i++] = 0;
        zap_sha512_block(c, c->buf);
        i = 0;
    }
    while (i < 120)
        c->buf[i++] = 0; /* high 64 bits of the 128-bit length are zero */
    for (int j = 7; j >= 0; j--)
        c->buf[i++] = (uint8_t)(bits >> (j * 8));
    zap_sha512_block(c, c->buf);
    for (int k = 0; k < 8; k++) {
        for (int j = 0; j < 8; j++)
            out[k * 8 + j] = (uint8_t)(c->h[k] >> ((7 - j) * 8));
    }
}

/* ---- SHA-1 (FIPS 180-1) ---- */
typedef struct {
    uint32_t h[5];
    uint64_t len;
    uint8_t buf[64];
    size_t n;
} zap_sha1;

static void zap_sha1_block(zap_sha1 *c, const uint8_t *p) {
    uint32_t w[80], a, b, cc, d, e, t;
    for (int i = 0; i < 16; i++) {
        w[i] = ((uint32_t)p[i * 4] << 24) | ((uint32_t)p[i * 4 + 1] << 16) |
               ((uint32_t)p[i * 4 + 2] << 8) | (uint32_t)p[i * 4 + 3];
    }
    for (int i = 16; i < 80; i++) {
        uint32_t v = w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16];
        w[i] = ZAP_ROL32(v, 1);
    }
    a = c->h[0];
    b = c->h[1];
    cc = c->h[2];
    d = c->h[3];
    e = c->h[4];
    for (int i = 0; i < 80; i++) {
        uint32_t f, k;
        if (i < 20) {
            f = (b & cc) | (~b & d);
            k = 0x5a827999;
        } else if (i < 40) {
            f = b ^ cc ^ d;
            k = 0x6ed9eba1;
        } else if (i < 60) {
            f = (b & cc) | (b & d) | (cc & d);
            k = 0x8f1bbcdc;
        } else {
            f = b ^ cc ^ d;
            k = 0xca62c1d6;
        }
        t = ZAP_ROL32(a, 5) + f + e + k + w[i];
        e = d;
        d = cc;
        cc = ZAP_ROL32(b, 30);
        b = a;
        a = t;
    }
    c->h[0] += a;
    c->h[1] += b;
    c->h[2] += cc;
    c->h[3] += d;
    c->h[4] += e;
}

static void zap_sha1_init(zap_sha1 *c) {
    c->h[0] = 0x67452301;
    c->h[1] = 0xefcdab89;
    c->h[2] = 0x98badcfe;
    c->h[3] = 0x10325476;
    c->h[4] = 0xc3d2e1f0;
    c->len = 0;
    c->n = 0;
}

static void zap_sha1_update(zap_sha1 *c, const uint8_t *d, size_t n) {
    c->len += n;
    for (size_t i = 0; i < n; i++) {
        c->buf[c->n++] = d[i];
        if (c->n == 64) {
            zap_sha1_block(c, c->buf);
            c->n = 0;
        }
    }
}

static void zap_sha1_final(zap_sha1 *c, uint8_t *out) {
    uint64_t bits = c->len * 8;
    size_t i = c->n;
    c->buf[i++] = 0x80;
    if (i > 56) {
        while (i < 64)
            c->buf[i++] = 0;
        zap_sha1_block(c, c->buf);
        i = 0;
    }
    while (i < 56)
        c->buf[i++] = 0;
    for (int j = 7; j >= 0; j--)
        c->buf[i++] = (uint8_t)(bits >> (j * 8));
    zap_sha1_block(c, c->buf);
    for (int k = 0; k < 5; k++) {
        out[k * 4] = (uint8_t)(c->h[k] >> 24);
        out[k * 4 + 1] = (uint8_t)(c->h[k] >> 16);
        out[k * 4 + 2] = (uint8_t)(c->h[k] >> 8);
        out[k * 4 + 3] = (uint8_t)c->h[k];
    }
}

/* ---- MD5 (RFC 1321) — little-endian ---- */
typedef struct {
    uint32_t h[4];
    uint64_t len;
    uint8_t buf[64];
    size_t n;
} zap_md5;

static void zap_md5_block(zap_md5 *ctx, const uint8_t *p) {
    static const uint32_t K[64] = {
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a,
        0xa8304613, 0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
        0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340,
        0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8,
        0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
        0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92,
        0xffeff47d, 0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
        0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391};
    static const uint8_t S[64] = {
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9,  14, 20, 5, 9,  14, 20, 5, 9,  14, 20, 5, 9,  14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21};
    uint32_t m[16];
    for (int i = 0; i < 16; i++) {
        m[i] = (uint32_t)p[i * 4] | ((uint32_t)p[i * 4 + 1] << 8) |
               ((uint32_t)p[i * 4 + 2] << 16) | ((uint32_t)p[i * 4 + 3] << 24);
    }
    uint32_t a = ctx->h[0], b = ctx->h[1], c = ctx->h[2], d = ctx->h[3];
    for (int i = 0; i < 64; i++) {
        uint32_t f, g;
        if (i < 16) {
            f = (b & c) | (~b & d);
            g = (uint32_t)i;
        } else if (i < 32) {
            f = (d & b) | (~d & c);
            g = (uint32_t)(5 * i + 1) & 15;
        } else if (i < 48) {
            f = b ^ c ^ d;
            g = (uint32_t)(3 * i + 5) & 15;
        } else {
            f = c ^ (b | ~d);
            g = (uint32_t)(7 * i) & 15;
        }
        uint32_t tmp = d;
        d = c;
        c = b;
        uint32_t x = a + f + K[i] + m[g];
        b = b + ZAP_ROL32(x, S[i]);
        a = tmp;
    }
    ctx->h[0] += a;
    ctx->h[1] += b;
    ctx->h[2] += c;
    ctx->h[3] += d;
}

static void zap_md5_init(zap_md5 *c) {
    c->h[0] = 0x67452301;
    c->h[1] = 0xefcdab89;
    c->h[2] = 0x98badcfe;
    c->h[3] = 0x10325476;
    c->len = 0;
    c->n = 0;
}

static void zap_md5_update(zap_md5 *c, const uint8_t *d, size_t n) {
    c->len += n;
    for (size_t i = 0; i < n; i++) {
        c->buf[c->n++] = d[i];
        if (c->n == 64) {
            zap_md5_block(c, c->buf);
            c->n = 0;
        }
    }
}

static void zap_md5_final(zap_md5 *c, uint8_t *out) {
    uint64_t bits = c->len * 8;
    size_t i = c->n;
    c->buf[i++] = 0x80;
    if (i > 56) {
        while (i < 64)
            c->buf[i++] = 0;
        zap_md5_block(c, c->buf);
        i = 0;
    }
    while (i < 56)
        c->buf[i++] = 0;
    for (int j = 0; j < 8; j++)
        c->buf[i++] = (uint8_t)(bits >> (j * 8)); /* little-endian length */
    zap_md5_block(c, c->buf);
    for (int k = 0; k < 4; k++) {
        out[k * 4] = (uint8_t)c->h[k];
        out[k * 4 + 1] = (uint8_t)(c->h[k] >> 8);
        out[k * 4 + 2] = (uint8_t)(c->h[k] >> 16);
        out[k * 4 + 3] = (uint8_t)(c->h[k] >> 24);
    }
}

/* ---- Unified, algorithm-tagged hash context ---- */
typedef struct {
    int32_t algo;
    union {
        zap_sha256 s256;
        zap_sha512 s512;
        zap_sha1 s1;
        zap_md5 m5;
    } u;
} zap_hash_ctx;

static size_t zap_crypto_digest_len(int algo) {
    switch (algo) {
    case ZAP_CRY_SHA256:
        return 32;
    case ZAP_CRY_SHA512:
        return 64;
    case ZAP_CRY_SHA1:
        return 20;
    case ZAP_CRY_MD5:
        return 16;
    default:
        return 0;
    }
}

static size_t zap_crypto_block_len(int algo) {
    return algo == ZAP_CRY_SHA512 ? 128 : 64;
}

static void zap_hash_ctx_init(zap_hash_ctx *c, int algo) {
    c->algo = algo;
    switch (algo) {
    case ZAP_CRY_SHA256:
        zap_sha256_init(&c->u.s256);
        break;
    case ZAP_CRY_SHA512:
        zap_sha512_init(&c->u.s512);
        break;
    case ZAP_CRY_SHA1:
        zap_sha1_init(&c->u.s1);
        break;
    default:
        zap_md5_init(&c->u.m5);
        break;
    }
}

static void zap_hash_ctx_update(zap_hash_ctx *c, const uint8_t *d, size_t n) {
    switch (c->algo) {
    case ZAP_CRY_SHA256:
        zap_sha256_update(&c->u.s256, d, n);
        break;
    case ZAP_CRY_SHA512:
        zap_sha512_update(&c->u.s512, d, n);
        break;
    case ZAP_CRY_SHA1:
        zap_sha1_update(&c->u.s1, d, n);
        break;
    default:
        zap_md5_update(&c->u.m5, d, n);
        break;
    }
}

/* Finalize a COPY so the caller's context stays usable (non-destructive). */
static void zap_hash_ctx_final(const zap_hash_ctx *c, uint8_t *out) {
    zap_hash_ctx t = *c;
    switch (t.algo) {
    case ZAP_CRY_SHA256:
        zap_sha256_final(&t.u.s256, out);
        break;
    case ZAP_CRY_SHA512:
        zap_sha512_final(&t.u.s512, out);
        break;
    case ZAP_CRY_SHA1:
        zap_sha1_final(&t.u.s1, out);
        break;
    default:
        zap_md5_final(&t.u.m5, out);
        break;
    }
}

/* Build a lowercase-hex ZapStr from `n` raw bytes. */
static void *zap_crypto_hex_str(const uint8_t *d, size_t n) {
    static const char HEX[] = "0123456789abcdef";
    ZapStr *s = (ZapStr *)zap_obj(&zap_desc_strobj);
    int64_t outlen = (int64_t)(n * 2);
    char *buf = (char *)zap_buf(outlen > 0 ? outlen : 1);
    for (size_t i = 0; i < n; i++) {
        buf[2 * i] = HEX[d[i] >> 4];
        buf[2 * i + 1] = HEX[d[i] & 0x0f];
    }
    s->len = outlen;
    s->data = buf;
    return s;
}

/* The hash context behind a streaming handle (a `bytes` ZapStr whose data
 * buffer is the context). */
static zap_hash_ctx *zap_crypto_handle_ctx(void *h) {
    return (zap_hash_ctx *)(uintptr_t)((ZapStr *)h)->data;
}

static void *zap_crypto_oneshot_hex(int algo, void *data) {
    ZapStr *d = (ZapStr *)data;
    zap_hash_ctx c;
    zap_hash_ctx_init(&c, algo);
    zap_hash_ctx_update(&c, (const uint8_t *)d->data, (size_t)d->len);
    uint8_t out[64];
    zap_hash_ctx_final(&c, out);
    return zap_crypto_hex_str(out, zap_crypto_digest_len(algo));
}

/* One-shot hex digests over a str/bytes value. */
void *zap_crypto_sha256(void *data) {
    return zap_crypto_oneshot_hex(ZAP_CRY_SHA256, data);
}
void *zap_crypto_sha512(void *data) {
    return zap_crypto_oneshot_hex(ZAP_CRY_SHA512, data);
}
void *zap_crypto_sha1(void *data) {
    return zap_crypto_oneshot_hex(ZAP_CRY_SHA1, data);
}
void *zap_crypto_md5(void *data) {
    return zap_crypto_oneshot_hex(ZAP_CRY_MD5, data);
}

/* Streaming: a `bytes` handle wrapping a heap-allocated, pointer-free context. */
void *zap_crypto_new(int64_t algo) {
    ZapStr *h = (ZapStr *)zap_obj(&zap_desc_strobj);
    zap_hash_ctx *c = (zap_hash_ctx *)zap_buf((int64_t)sizeof(zap_hash_ctx));
    zap_hash_ctx_init(c, (int)algo);
    h->len = (int64_t)sizeof(zap_hash_ctx);
    h->data = (const char *)c;
    return h;
}

void zap_crypto_update(void *handle, void *data) {
    zap_hash_ctx *c = zap_crypto_handle_ctx(handle);
    ZapStr *d = (ZapStr *)data;
    zap_hash_ctx_update(c, (const uint8_t *)d->data, (size_t)d->len);
}

void *zap_crypto_hexdigest(void *handle) {
    zap_hash_ctx *c = zap_crypto_handle_ctx(handle);
    uint8_t out[64];
    zap_hash_ctx_final(c, out);
    return zap_crypto_hex_str(out, zap_crypto_digest_len(c->algo));
}

void *zap_crypto_digest(void *handle) {
    zap_hash_ctx *c = zap_crypto_handle_ctx(handle);
    uint8_t out[64];
    size_t n = zap_crypto_digest_len(c->algo);
    zap_hash_ctx_final(c, out);
    return zap_str_new((const char *)out, (int64_t)n);
}

/* ---- HMAC (RFC 2104) ---- */
static void zap_crypto_hmac(int algo, const uint8_t *key, size_t keylen,
                            const uint8_t *msg, size_t msglen, uint8_t *out) {
    size_t blk = zap_crypto_block_len(algo);
    size_t dlen = zap_crypto_digest_len(algo);
    uint8_t k[128];
    memset(k, 0, sizeof(k));
    if (keylen > blk) {
        zap_hash_ctx c;
        zap_hash_ctx_init(&c, algo);
        zap_hash_ctx_update(&c, key, keylen);
        zap_hash_ctx_final(&c, k);
    } else {
        memcpy(k, key, keylen);
    }
    uint8_t ipad[128], opad[128];
    for (size_t i = 0; i < blk; i++) {
        ipad[i] = (uint8_t)(k[i] ^ 0x36);
        opad[i] = (uint8_t)(k[i] ^ 0x5c);
    }
    uint8_t inner[64];
    zap_hash_ctx c;
    zap_hash_ctx_init(&c, algo);
    zap_hash_ctx_update(&c, ipad, blk);
    zap_hash_ctx_update(&c, msg, msglen);
    zap_hash_ctx_final(&c, inner);
    zap_hash_ctx_init(&c, algo);
    zap_hash_ctx_update(&c, opad, blk);
    zap_hash_ctx_update(&c, inner, dlen);
    zap_hash_ctx_final(&c, out);
}

static void *zap_crypto_hmac_hex(int algo, void *key, void *msg) {
    ZapStr *k = (ZapStr *)key;
    ZapStr *m = (ZapStr *)msg;
    uint8_t out[64];
    zap_crypto_hmac(algo, (const uint8_t *)k->data, (size_t)k->len,
                    (const uint8_t *)m->data, (size_t)m->len, out);
    return zap_crypto_hex_str(out, zap_crypto_digest_len(algo));
}

void *zap_crypto_hmac_sha256(void *key, void *msg) {
    return zap_crypto_hmac_hex(ZAP_CRY_SHA256, key, msg);
}
void *zap_crypto_hmac_sha512(void *key, void *msg) {
    return zap_crypto_hmac_hex(ZAP_CRY_SHA512, key, msg);
}

/* ---- Secure random / secrets ---- */
static void zap_crypto_fill_random(uint8_t *buf, size_t n) {
    size_t off = 0;
    while (off < n) {
        size_t chunk = n - off;
        if (chunk > 256)
            chunk = 256; /* getentropy's per-call limit */
        if (getentropy(buf + off, chunk) == 0) {
            off += chunk;
            continue;
        }
        /* Fallback: read the kernel CSPRNG directly. */
        int fd = open("/dev/urandom", O_RDONLY);
        if (fd < 0) {
            fprintf(stderr, "zap: crypto: no CSPRNG available\n");
            abort();
        }
        while (off < n) {
            ssize_t r = read(fd, buf + off, n - off);
            if (r <= 0) {
                fprintf(stderr, "zap: crypto: CSPRNG read failed\n");
                abort();
            }
            off += (size_t)r;
        }
        close(fd);
        return;
    }
}

void *zap_crypto_random_bytes(int64_t n) {
    if (n < 0)
        n = 0;
    ZapStr *s = (ZapStr *)zap_obj(&zap_desc_strobj);
    uint8_t *buf = (uint8_t *)zap_buf(n > 0 ? n : 1);
    if (n > 0)
        zap_crypto_fill_random(buf, (size_t)n);
    s->len = n;
    s->data = (const char *)buf;
    return s;
}

void *zap_crypto_token_hex(int64_t n) {
    if (n < 0)
        n = 0;
    uint8_t *rnd = (uint8_t *)zap_buf(n > 0 ? n : 1);
    if (n > 0)
        zap_crypto_fill_random(rnd, (size_t)n);
    return zap_crypto_hex_str(rnd, (size_t)n);
}

/* ===========================================================================
 * time & datetime (ticket #99, D-0056). Clocks, sleep, duration-unit helpers,
 * and civil-time conversion. Durations are plain int64 nanoseconds (Go
 * time.Duration style), so arithmetic needs no runtime support; the helpers
 * here just convert between a unit and nanoseconds. Civil time uses the C
 * library's gmtime_r/localtime_r/timegm, so timezone support is UTC + local
 * only (a full tz database is a follow-up, see #99). <time.h>/<unistd.h>/
 * <errno.h> are included near the top of this file.
 * =========================================================================== */

/* --- clocks --- */
int64_t zap_time_monotonic_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}
double zap_time_monotonic_seconds(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}
int64_t zap_time_wall_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}
double zap_time_wall_seconds(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}

/* --- sleep ---
 * v1 is a plain blocking nanosleep (EINTR-resuming): it blocks the current OS
 * thread, so under the M:N scheduler it parks that M rather than yielding
 * cooperatively. A scheduler-aware sleep (via the netpoller timer) is a
 * follow-up (#99). */
void zap_time_sleep_ns(int64_t ns) {
    if (ns <= 0) {
        return;
    }
    struct timespec req;
    req.tv_sec = (time_t)(ns / 1000000000LL);
    req.tv_nsec = (long)(ns % 1000000000LL);
    struct timespec rem;
    while (nanosleep(&req, &rem) != 0 && errno == EINTR) {
        req = rem;
    }
}
void zap_time_sleep_seconds(double s) {
    if (s <= 0.0) {
        return;
    }
    zap_time_sleep_ns((int64_t)(s * 1e9));
}

/* --- duration unit constructors (-> nanoseconds) --- */
int64_t zap_time_seconds_to_ns(int64_t s) { return s * 1000000000LL; }
int64_t zap_time_millis_to_ns(int64_t ms) { return ms * 1000000LL; }
int64_t zap_time_micros_to_ns(int64_t us) { return us * 1000LL; }

/* --- duration unit converters (nanoseconds -> unit as float) --- */
double zap_time_ns_to_seconds(int64_t ns) { return (double)ns / 1e9; }
double zap_time_ns_to_millis(int64_t ns) { return (double)ns / 1e6; }
double zap_time_ns_to_micros(int64_t ns) { return (double)ns / 1e3; }

/* --- civil (calendar) time ---
 * `is_local` selects local time (localtime_r) over UTC (gmtime_r). */
static void zap_civil(int64_t ts, int64_t is_local, struct tm *out) {
    time_t t = (time_t)ts;
    if (is_local) {
        localtime_r(&t, out);
    } else {
        gmtime_r(&t, out);
    }
}
int64_t zap_datetime_year(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_year + 1900;
}
int64_t zap_datetime_month(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_mon + 1;
}
int64_t zap_datetime_day(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_mday;
}
int64_t zap_datetime_hour(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_hour;
}
int64_t zap_datetime_minute(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_min;
}
int64_t zap_datetime_second(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_sec;
}

int64_t zap_datetime_now_unix(void) { return (int64_t)time(NULL); }

/* Civil components (UTC) -> Unix timestamp. Uses timegm (glibc extension; the
 * CI toolchain provides it) so the components are interpreted as UTC. */
int64_t zap_datetime_to_unix(int64_t y, int64_t mo, int64_t d, int64_t h,
                             int64_t mi, int64_t s) {
    struct tm tm;
    memset(&tm, 0, sizeof tm);
    tm.tm_year = (int)(y - 1900);
    tm.tm_mon = (int)(mo - 1);
    tm.tm_mday = (int)d;
    tm.tm_hour = (int)h;
    tm.tm_min = (int)mi;
    tm.tm_sec = (int)s;
    return (int64_t)timegm(&tm);
}

/* Civil components -> ISO-8601 "YYYY-MM-DDTHH:MM:SS" (returns a fresh ZapStr*). */
void *zap_datetime_format_iso(int64_t y, int64_t mo, int64_t d, int64_t h,
                              int64_t mi, int64_t s) {
    char buf[32];
    int n = snprintf(buf, sizeof buf, "%04lld-%02lld-%02lldT%02lld:%02lld:%02lld",
                     (long long)y, (long long)mo, (long long)d, (long long)h,
                     (long long)mi, (long long)s);
    if (n < 0) {
        n = 0;
    }
    return zap_str_new(buf, (int64_t)n);
}

/* ISO-8601 string -> Unix timestamp (UTC). Accepts a date ("YYYY-MM-DD") or a
 * full date-time with a 'T' or ' ' separator; fields left unmatched default to
 * the start of the day. v1 parsing is lenient (sscanf): malformed input yields
 * a best-effort timestamp rather than an error (strict parsing is a follow-up,
 * #99). */
int64_t zap_datetime_parse_iso(void *s) {
    ZapStr *z = (ZapStr *)s;
    char buf[64];
    int64_t n = z->len;
    if (n < 0) {
        n = 0;
    }
    if (n > 63) {
        n = 63;
    }
    memcpy(buf, z->data, (size_t)n);
    buf[n] = '\0';
    int y = 1970, mo = 1, d = 1, h = 0, mi = 0, sec = 0;
    sscanf(buf, "%d-%d-%d%*c%d:%d:%d", &y, &mo, &d, &h, &mi, &sec);
    return zap_datetime_to_unix(y, mo, d, h, mi, sec);
}

/* ===========================================================================
 * Type descriptors for the runtime's own heap types (ticket #48). Both GC
 * builds read `.size`; the precise Immix collector also reads `.kind` and the
 * pointer-offset arrays. Codegen emits the descriptors for *user* structs and
 * closures; these cover the built-in runtime objects. See design/0008-precise-gc.
 * ========================================================================== */

static const int32_t zap_off_strobj[] = {(int32_t)offsetof(ZapStr, data)};
static const int32_t zap_off_gobj[] = {
    (int32_t)offsetof(G, stack),
    (int32_t)offsetof(G, arg),
    (int32_t)offsetof(G, scope),
};
static const int32_t zap_off_sigsub[] = {
    (int32_t)offsetof(SigSub, next),
    (int32_t)offsetof(SigSub, chan),
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
const ZapTypeDesc zap_desc_sigsub = {(int64_t)sizeof(SigSub), ZAP_TD_PRECISE, 2,
                                     zap_off_sigsub};

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
    zap_gc_add_root_fn(zap_sig_mark_roots); /* ticket #77: notify subscribers    */
}

/* The precise Immix collector implementation. Included last so it sees every
 * runtime type and the scheduler globals above. */
#include "zapgc.inc.c"
#endif

/* ===========================================================================
 * stdlib: math + random (ticket #98, D-0056)
 * ---------------------------------------------------------------------------
 * `math` — thin libm wrappers (unconditionally compiled for both collectors).
 * `random` — a seedable xoshiro256** generator with SplitMix64 seed expansion,
 * autoseeded from OS entropy (getentropy / arc4random) on first use. A single
 * mutex guards the global state so concurrent goroutines never observe torn
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
    pthread_mutex_lock(&zap_rng_mu);
    zap_rng_seed_locked((uint64_t)n);
    pthread_mutex_unlock(&zap_rng_mu);
}

/* A double in [0, 1): the top 53 bits scaled by 2^-53. */
double zap_random_f64(void) {
    pthread_mutex_lock(&zap_rng_mu);
    zap_rng_ensure_locked();
    uint64_t x = zap_rng_next_locked();
    pthread_mutex_unlock(&zap_rng_mu);
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
    pthread_mutex_lock(&zap_rng_mu);
    zap_rng_ensure_locked();
    uint64_t off = (span == 0ULL) /* a..b spans the full int64 width */
                       ? zap_rng_next_locked()
                       : zap_rng_bounded_locked(span);
    pthread_mutex_unlock(&zap_rng_mu);
    return (int64_t)((uint64_t)a + off);
}

/* A random valid index into a non-empty list (backs `random.choice`). */
int64_t zap_random_list_index(void *l) {
    ZapList *list = (ZapList *)l;
    if (list->len <= 0) {
        fprintf(stderr, "zap: random.choice from an empty sequence\n");
        abort();
    }
    pthread_mutex_lock(&zap_rng_mu);
    zap_rng_ensure_locked();
    uint64_t idx = zap_rng_bounded_locked((uint64_t)list->len);
    pthread_mutex_unlock(&zap_rng_mu);
    return (int64_t)idx;
}

/* Fisher-Yates in place over the element words (works for any element kind;
 * pointer words keep their GC identity, only their positions change). */
void zap_random_shuffle(void *l) {
    ZapList *list = (ZapList *)l;
    pthread_mutex_lock(&zap_rng_mu);
    zap_rng_ensure_locked();
    for (int64_t i = list->len - 1; i > 0; i--) {
        uint64_t j = zap_rng_bounded_locked((uint64_t)(i + 1));
        int64_t tmp = list->data[i];
        list->data[i] = list->data[(int64_t)j];
        list->data[(int64_t)j] = tmp;
    }
    pthread_mutex_unlock(&zap_rng_mu);
}

/* ===========================================================================
 * json — JSON tokenizer cursor (ticket #100 / #131, D-0059)
 *
 * A pull lexer over an in-memory str/bytes source. The high-level tree build
 * (json.loads) and serialization (json.dumps) live in Zap prelude functions
 * (see PRELUDE_SRC) that drive this cursor through the internal `json._lex_*`
 * seam primitives — so JSON<->JsonValue construction stays in compiler-emitted
 * code and this C never needs to know the JsonValue struct layout.
 *
 * The cursor is a pointer-free leaf (zap_buf): it keeps a raw pointer into the
 * source ZapStr's byte buffer (the caller keeps the source live across the
 * parse), plus the current token's kind and decoded scalar value.
 * ======================================================================== */
#include <errno.h>

enum {
    ZAP_JTOK_END = 0,
    ZAP_JTOK_NULL = 1,
    ZAP_JTOK_TRUE = 2,
    ZAP_JTOK_FALSE = 3,
    ZAP_JTOK_INT = 4,
    ZAP_JTOK_FLOAT = 5,
    ZAP_JTOK_STR = 6,
    ZAP_JTOK_LBRACE = 7,
    ZAP_JTOK_RBRACE = 8,
    ZAP_JTOK_LBRACKET = 9,
    ZAP_JTOK_RBRACKET = 10,
    ZAP_JTOK_COLON = 11,
    ZAP_JTOK_COMMA = 12,
    ZAP_JTOK_ERR = -1
};

typedef struct {
    const char *data;
    int64_t len;
    int64_t pos;
    int64_t kind;      /* current token kind (ZAP_JTOK_*) */
    int64_t ival;      /* ZAP_JTOK_INT value */
    double fval;       /* ZAP_JTOK_FLOAT value */
    const char *sdata; /* ZAP_JTOK_STR: start of raw content (inside the quotes) */
    int64_t slen;      /* ZAP_JTOK_STR: raw content length */
    int64_t has_esc;   /* ZAP_JTOK_STR: content contains backslash escapes */
} ZapJsonLex;

/* Open a cursor over a str/bytes source (a ZapStr*). */
void *zap_json_lex_new(void *s) {
    ZapStr *z = (ZapStr *)s;
    ZapJsonLex *lx = (ZapJsonLex *)zap_buf((int64_t)sizeof(ZapJsonLex));
    lx->data = z->data;
    lx->len = z->len;
    lx->pos = 0;
    lx->kind = ZAP_JTOK_END;
    lx->ival = 0;
    lx->fval = 0.0;
    lx->sdata = NULL;
    lx->slen = 0;
    lx->has_esc = 0;
    return lx;
}

static int zap_json_is_ws(char c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r';
}

/* Advance to and classify the next token; returns (and stores) its kind. */
int64_t zap_json_lex_next(void *h) {
    ZapJsonLex *lx = (ZapJsonLex *)h;
    const char *d = lx->data;
    int64_t n = lx->len;
    int64_t i = lx->pos;
    while (i < n && zap_json_is_ws(d[i])) {
        i++;
    }
    if (i >= n) {
        lx->pos = i;
        lx->kind = ZAP_JTOK_END;
        return ZAP_JTOK_END;
    }
    char c = d[i];
    switch (c) {
    case '{': lx->pos = i + 1; lx->kind = ZAP_JTOK_LBRACE; return lx->kind;
    case '}': lx->pos = i + 1; lx->kind = ZAP_JTOK_RBRACE; return lx->kind;
    case '[': lx->pos = i + 1; lx->kind = ZAP_JTOK_LBRACKET; return lx->kind;
    case ']': lx->pos = i + 1; lx->kind = ZAP_JTOK_RBRACKET; return lx->kind;
    case ':': lx->pos = i + 1; lx->kind = ZAP_JTOK_COLON; return lx->kind;
    case ',': lx->pos = i + 1; lx->kind = ZAP_JTOK_COMMA; return lx->kind;
    case '"': {
        int64_t start = i + 1;
        int64_t j = start;
        int64_t esc = 0;
        while (j < n && d[j] != '"') {
            if (d[j] == '\\') {
                esc = 1;
                j += (j + 1 < n) ? 2 : 1;
            } else {
                j++;
            }
        }
        lx->sdata = d + start;
        lx->slen = (j <= n ? j : n) - start;
        lx->has_esc = esc;
        lx->kind = ZAP_JTOK_STR;
        lx->pos = (j < n) ? j + 1 : n; /* consume the closing quote */
        return lx->kind;
    }
    default:
        break;
    }
    if (c == 'n' && i + 4 <= n && memcmp(d + i, "null", 4) == 0) {
        lx->pos = i + 4;
        lx->kind = ZAP_JTOK_NULL;
        return lx->kind;
    }
    if (c == 't' && i + 4 <= n && memcmp(d + i, "true", 4) == 0) {
        lx->pos = i + 4;
        lx->kind = ZAP_JTOK_TRUE;
        return lx->kind;
    }
    if (c == 'f' && i + 5 <= n && memcmp(d + i, "false", 5) == 0) {
        lx->pos = i + 5;
        lx->kind = ZAP_JTOK_FALSE;
        return lx->kind;
    }
    if (c == '-' || (c >= '0' && c <= '9')) {
        int64_t start = i;
        int is_float = 0;
        if (d[i] == '-') {
            i++;
        }
        while (i < n && d[i] >= '0' && d[i] <= '9') {
            i++;
        }
        if (i < n && d[i] == '.') {
            is_float = 1;
            i++;
            while (i < n && d[i] >= '0' && d[i] <= '9') {
                i++;
            }
        }
        if (i < n && (d[i] == 'e' || d[i] == 'E')) {
            is_float = 1;
            i++;
            if (i < n && (d[i] == '+' || d[i] == '-')) {
                i++;
            }
            while (i < n && d[i] >= '0' && d[i] <= '9') {
                i++;
            }
        }
        int64_t tlen = i - start;
        char buf[80];
        int64_t cl = tlen < 79 ? tlen : 79;
        memcpy(buf, d + start, (size_t)cl);
        buf[cl] = 0;
        if (!is_float) {
            errno = 0;
            char *end = NULL;
            long long v = strtoll(buf, &end, 10);
            if (errno == 0 && end == buf + cl) {
                lx->ival = (int64_t)v;
                lx->kind = ZAP_JTOK_INT;
                lx->pos = i;
                return lx->kind;
            }
            /* overflow / out-of-i64: fall back to float (D-0059) */
        }
        lx->fval = strtod(buf, NULL);
        lx->kind = ZAP_JTOK_FLOAT;
        lx->pos = i;
        return lx->kind;
    }
    lx->pos = i + 1;
    lx->kind = ZAP_JTOK_ERR;
    return lx->kind;
}

int64_t zap_json_lex_int(void *h) { return ((ZapJsonLex *)h)->ival; }
double zap_json_lex_float(void *h) { return ((ZapJsonLex *)h)->fval; }

/* Build a ZapStr from the current STR token, decoding escapes if present.
 * Escapes never expand (\uXXXX -> at most 3 UTF-8 bytes from 6 source bytes),
 * so the raw length is a safe output capacity. */
void *zap_json_lex_str(void *h) {
    ZapJsonLex *lx = (ZapJsonLex *)h;
    const char *s = lx->sdata;
    int64_t n = lx->slen;
    if (!lx->has_esc) {
        return zap_str_new(s, n);
    }
    char *out = (char *)zap_buf(n > 0 ? n : 1);
    int64_t o = 0;
    for (int64_t i = 0; i < n; i++) {
        char c = s[i];
        if (c == '\\' && i + 1 < n) {
            char e = s[++i];
            switch (e) {
            case 'n': out[o++] = '\n'; break;
            case 't': out[o++] = '\t'; break;
            case 'r': out[o++] = '\r'; break;
            case 'b': out[o++] = '\b'; break;
            case 'f': out[o++] = '\f'; break;
            case '/': out[o++] = '/'; break;
            case '\\': out[o++] = '\\'; break;
            case '"': out[o++] = '"'; break;
            case 'u': {
                if (i + 4 < n) {
                    unsigned int cp = 0;
                    for (int k = 0; k < 4; k++) {
                        char hx = s[i + 1 + k];
                        cp <<= 4;
                        if (hx >= '0' && hx <= '9') {
                            cp |= (unsigned)(hx - '0');
                        } else if (hx >= 'a' && hx <= 'f') {
                            cp |= (unsigned)(hx - 'a' + 10);
                        } else if (hx >= 'A' && hx <= 'F') {
                            cp |= (unsigned)(hx - 'A' + 10);
                        }
                    }
                    i += 4;
                    if (cp < 0x80) {
                        out[o++] = (char)cp;
                    } else if (cp < 0x800) {
                        out[o++] = (char)(0xC0 | (cp >> 6));
                        out[o++] = (char)(0x80 | (cp & 0x3F));
                    } else {
                        out[o++] = (char)(0xE0 | (cp >> 12));
                        out[o++] = (char)(0x80 | ((cp >> 6) & 0x3F));
                        out[o++] = (char)(0x80 | (cp & 0x3F));
                    }
                }
                break;
            }
            default: out[o++] = e; break;
            }
        } else {
            out[o++] = c;
        }
    }
    return zap_str_new(out, o);
}

/* --- json.dumps helpers (ticket #100 / #131) ---------------------------------
 * The serializer (`__json_dumps`) is a Zap prelude walker that emits each
 * fragment into a list[str] and joins once; these C helpers cover the pieces
 * Zap lacks a direct surface for: enumerating an object's keys, JSON string
 * escaping, and the final concatenation. Object key order is unspecified (hash
 * order), matching D-0059. */

/* Collect a dict's keys (str/ptr keys) into a fresh list, for `for k in keys`. */
void *zap_dict_keys(void *dp) {
    ZapDict *d = (ZapDict *)dp;
    void *out = zap_list_new();
    for (int64_t i = 0; i < d->cap; i++) {
        if (d->entries[i].state == SLOT_FULL) {
            zap_list_push_ptr(out, (void *)(intptr_t)d->entries[i].key);
        }
    }
    return out;
}

/* JSON-escape a string and wrap it in double quotes (RFC 8259 §7). */
void *zap_json_escape(void *sp) {
    ZapStr *s = (ZapStr *)sp;
    int64_t n = s->len;
    const char *in = s->data;
    /* worst case per byte: \u00XX (6); plus the two surrounding quotes. */
    char *out = (char *)zap_buf(n * 6 + 2);
    int64_t o = 0;
    static const char hex[] = "0123456789abcdef";
    out[o++] = '"';
    for (int64_t i = 0; i < n; i++) {
        unsigned char c = (unsigned char)in[i];
        switch (c) {
        case '"': out[o++] = '\\'; out[o++] = '"'; break;
        case '\\': out[o++] = '\\'; out[o++] = '\\'; break;
        case '\n': out[o++] = '\\'; out[o++] = 'n'; break;
        case '\t': out[o++] = '\\'; out[o++] = 't'; break;
        case '\r': out[o++] = '\\'; out[o++] = 'r'; break;
        case '\b': out[o++] = '\\'; out[o++] = 'b'; break;
        case '\f': out[o++] = '\\'; out[o++] = 'f'; break;
        default:
            if (c < 0x20) {
                out[o++] = '\\';
                out[o++] = 'u';
                out[o++] = '0';
                out[o++] = '0';
                out[o++] = hex[(c >> 4) & 0xF];
                out[o++] = hex[c & 0xF];
            } else {
                out[o++] = (char)c;
            }
        }
    }
    out[o++] = '"';
    return zap_str_new(out, o);
}

/* Concatenate every ZapStr* element of a list into one string. */
void *zap_json_join(void *lp) {
    ZapList *l = (ZapList *)lp;
    int64_t total = 0;
    for (int64_t i = 0; i < l->len; i++) {
        ZapStr *part = (ZapStr *)zap_list_get_ptr(lp, i);
        total += part->len;
    }
    char *out = (char *)zap_buf(total > 0 ? total : 1);
    int64_t o = 0;
    for (int64_t i = 0; i < l->len; i++) {
        ZapStr *part = (ZapStr *)zap_list_get_ptr(lp, i);
        if (part->len > 0) {
            memcpy(out + o, part->data, (size_t)part->len);
            o += part->len;
        }
    }
    return zap_str_new(out, o);
}
