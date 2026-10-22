/* =============================================================================
 * Precise, Immix-style garbage collector (ticket #48, D-0009,
 * design/0008-precise-gc.md).
 *
 * This file is #included at the BOTTOM of zaprt.c when the program is built with
 * the `gc-immix` codegen feature (which defines ZAP_GC_IMMIX and omits <gc.h> /
 * -lgc). It sees every runtime type (ZapStr, ZapList, ..., the scheduler G/P/M,
 * Scope) and the scheduler globals (g_all, cur_m, ...) defined above it; the
 * entry points it provides are forward-declared at the top of zaprt.c.
 *
 * v1 is NON-MOVING: a line-marking mark-sweep over an Immix heap (32 KiB blocks
 * / 128 B lines, bump allocation, line-granularity recycling), with a separate
 * large-object space. Roots are found PRECISELY via a per-coroutine shadow stack
 * (so a parked coroutine's roots are just its saved chain) plus a global-root
 * registry; the heap is traced precisely from per-object type descriptors.
 * Stop-the-world is cooperative: mutators poll a safepoint and park.
 *
 * Allocation is lock-free on the fast path (ticket #119): each scheduler P
 * (realised here as a per-mutator buffer — M:P is 1:1 and pinned) owns an Immix
 * block it bump-allocates into, plus an overflow block for medium objects, so
 * the common case is a pointer bump with no lock. The global heap lock
 * (zap_heap_mu) is taken only on the slow paths: refilling an exhausted buffer,
 * large-object allocation, and the collector's sweep. At stop-the-world the
 * collector relinquishes every P's buffers (zap_reset_alloc_bufs) so their
 * blocks sweep normally and each P refills fresh on resume. See the design doc
 * for the rationale and the remaining deferred pieces (evacuation/defrag,
 * generational barrier).
 * ========================================================================== */

#include <sys/mman.h>

/* ---- geometry ------------------------------------------------------------- */
#define ZAP_BLOCK 32768u          /* bytes per block (power of two, aligned)    */
#define ZAP_LINE 128u             /* bytes per line                             */
#define ZAP_LINES (ZAP_BLOCK / ZAP_LINE) /* 256 lines per block                 */
#define ZAP_LOS_THRESHOLD 8192u   /* objects larger than this go to the LOS     */

/* A block's metadata lives at the block's base; the first `ZAP_RESERVED_LINES`
 * lines are reserved for it and never allocate objects. */
typedef struct ZapBlock {
    struct ZapBlock *next;        /* intrusive list link (free / recycle / all) */
    struct ZapBlock *all_next;    /* link through every block ever mapped       */
    uint8_t kind;                 /* ZAP_BK_SMALL or ZAP_BK_LOS                  */
    uint8_t los_mark;             /* LOS: 1 iff marked this cycle               */
    size_t los_size;              /* LOS: full object size incl. header         */
    uint32_t hole_cursor;         /* small: byte offset of next alloc           */
    uint32_t hole_limit;          /* small: end of current free run             */
    uint32_t search_line;         /* small: line to resume hole search from     */
    uint32_t avail;               /* small: free bytes when placed on a list    */
    uint32_t live_lines;          /* small: live lines at the last sweep (#120) */
    uint8_t evac_src;             /* small: selected evacuation source (#120)   */
    uint8_t line_mark[ZAP_LINES]; /* small: per-line liveness (set by tracing)  */
} ZapBlock;

enum { ZAP_BK_SMALL = 0, ZAP_BK_LOS = 1 };

#define ZAP_RESERVED_BYTES (((sizeof(ZapBlock) + ZAP_LINE - 1) / ZAP_LINE) * ZAP_LINE)
#define ZAP_RESERVED_LINES (ZAP_RESERVED_BYTES / ZAP_LINE)

/* The ZapTypeDesc typedef and the ZAP_TD_* kind codes are defined in zaprt.c
 * (shared with the Boehm build and with codegen's emitted descriptors). */

/* Every heap object is [ desc|mark (8) | payload_size (8) | payload... ]. The
 * user/runtime pointer is the PAYLOAD pointer. The descriptor word (payload[-2])
 * holds the 8-byte-aligned descriptor pointer OR'd with the mark bit in bit 0.
 * The size word (payload[-1]) holds the payload byte size, so line-marking knows
 * an object's extent even for variable-length atomic buffers whose descriptor is
 * shared. The 16-byte header also 16-byte-aligns every payload. */
#define ZAP_HEADER_BYTES 16u
#define ZAP_HDR(payload) (((uintptr_t *)(payload))[-2])
#define ZAP_OSIZE(payload) (((size_t *)(payload))[-1])
#define ZAP_DESC(payload) ((const ZapTypeDesc *)(ZAP_HDR(payload) & ~(uintptr_t)7))
#define ZAP_MARKED(payload) (ZAP_HDR(payload) & 1u)
#define ZAP_SET_MARK(payload) (ZAP_HDR(payload) |= 1u)
#define ZAP_CLR_MARK(payload) (ZAP_HDR(payload) &= ~(uintptr_t)1)

/* Evacuation forwarding (ticket #120). The descriptor pointer is 8-byte aligned,
 * so header bits 0..2 are free; bit 0 is the mark bit (above), bit 1 marks an
 * object that has been MOVED (forwarded). When an object is evacuated, its new
 * payload address is written into the OLD object's size-word header slot
 * (payload[-1]) — dead once the object has been copied, and always present (part
 * of the 16-byte header), so there is no minimum-payload-size constraint. Any
 * reference still pointing at the old object is rewritten to the target the next
 * time the collector processes the slot. The forwarding bit is only ever set on
 * a dead source-block object (reclaimed by the same sweep), so it never needs
 * clearing. */
#define ZAP_FWD_BIT 2u
#define ZAP_IS_FWD(payload) (ZAP_HDR(payload) & ZAP_FWD_BIT)
#define ZAP_FWD_TARGET(payload) (((void **)(payload))[-1])
#define ZAP_SET_FWD(payload, np)                                                                   \
    do {                                                                                           \
        ZAP_HDR(payload) |= ZAP_FWD_BIT;                                                           \
        ((void **)(payload))[-1] = (np);                                                           \
    } while (0)

/* ---- heap globals --------------------------------------------------------- */
static pthread_mutex_t zap_heap_mu = PTHREAD_MUTEX_INITIALIZER;
static ZapBlock *zap_free_blocks = NULL;    /* fully-free small blocks, reusable */
static ZapBlock *zap_recycle_blocks = NULL; /* small blocks with holes           */
static ZapBlock *zap_all_blocks = NULL;     /* every mapped block (sweep walks)   */
static size_t zap_bytes_since_gc = 0;
static size_t zap_live_estimate = 0;

/* ---- opportunistic evacuation / defragmentation (ticket #120) ------------- *
 * A moving second phase for the precise Immix collector. Immix fragments over
 * time: blocks accumulate holes as objects die at line granularity, leaving many
 * partially-occupied blocks that waste whole-block capacity. Evacuation relocates
 * the live objects out of the SPARSEST blocks into dense fresh blocks during a
 * collection, so the emptied source blocks are reclaimed whole and allocation
 * locality improves.
 *
 * Because the collector is PRECISE — it visits every root and every pointer field
 * — it can rewrite each reference to a moved object to its new address, so no
 * stale pointer survives. Compiled code observes the move because codegen reloads
 * every GC-pointer value from its shadow slot after a safepoint (load-at-use,
 * #120 in codegen/src/lib.rs); the collector updates those slots here.
 *
 * SAFETY / what moves. v1 moves only `zap_desc_atomic` leaf buffers (string
 * bytes, numeric array data, and the element/entry buffers of the built-in
 * containers) — the dominant source of fragmentation. They have no pointer
 * children, are reached only through a parent's pointer slot (updated here) or a
 * reloaded SSA value, and are never cached by raw C across a safepoint (the
 * runtime's C routines re-read the parent each call and never poll a safepoint
 * mid-operation). Everything else is marked in place: the scheduler-owned objects
 * the runtime holds by raw C pointer (the coroutine `G` and its green stack,
 * `Scope`, `ZapChan`, signal subscriptions, sockets, sync primitives, the LOS),
 * the CPython-interop wrapper (finalizer-registered), and any object a non-slot
 * root marks (the reflection and signal tables, via `zap_gc_mark`). Pinning is
 * achieved simply by marking those objects BEFORE any evacuation runs: an
 * already-marked object is never moved. The forwarding/update protocol itself is
 * fully general, so widening the movable set later needs no new machinery. */
#define ZAP_EVAC_MAX_SRC 64u /* cap source blocks per cycle (bounds copy + to-space) */
/* A small block is a candidate source when it is non-empty but at most this many
 * of its usable lines were live at the previous sweep (~<=50% occupancy). */
#define ZAP_EVAC_SPARSE_LINES (((ZAP_LINES - ZAP_RESERVED_LINES)) / 2u)
static int zap_evac_active = 0;        /* evacuation enabled for the current cycle */
static int zap_evac_disabled = 0;      /* ZAP_GC_NO_EVAC set: skip evacuation      */
static ZapBlock *zap_evac_tospace = NULL; /* current dense target block            */
static uint32_t zap_evac_cursor = 0;   /* bump offset within zap_evac_tospace      */
static size_t zap_evac_moved = 0;      /* objects relocated this cycle (stats)     */
static ZapBlock *zap_map_block(void);  /* fwd: map a fresh small block             */

/* ---- block registry ------------------------------------------------------- *
 * An open-addressing hash set of block base addresses, so the marker can decide
 * in O(1) whether an arbitrary pointer addresses one of our heap blocks. This
 * makes rooting robust: codegen roots every pointer-TYPED SSA value, but some of
 * those are not GC-heap pointers (stack allocas, function addresses, inline
 * value-struct pointers). A non-heap pointer's computed block base is not in the
 * registry, so the marker safely ignores it. In precise mode (no conservative
 * scanning) every real GC pointer addresses an object START, so membership +
 * 16-byte alignment is sufficient to accept it; a reachable object is never
 * freed-then-reused underneath a live pointer, so no stale pointer can alias a
 * different live object. */
static uintptr_t *zap_reg = NULL; /* table of block bases (0 = empty, 1 = tomb) */
static size_t zap_reg_cap = 0;
static size_t zap_reg_len = 0;

static void zap_reg_insert(uintptr_t base);

static void zap_reg_grow(void) {
    size_t ncap = zap_reg_cap ? zap_reg_cap * 2 : 256;
    uintptr_t *old = zap_reg;
    size_t ocap = zap_reg_cap;
    zap_reg = (uintptr_t *)calloc(ncap, sizeof(uintptr_t));
    zap_reg_cap = ncap;
    zap_reg_len = 0;
    for (size_t i = 0; i < ocap; i++) {
        if (old[i] > 1) {
            zap_reg_insert(old[i]);
        }
    }
    free(old);
}

static void zap_reg_insert(uintptr_t base) {
    if ((zap_reg_len + 1) * 10 >= zap_reg_cap * 7) {
        zap_reg_grow();
    }
    size_t mask = zap_reg_cap - 1;
    size_t i = (base >> 15) & mask;
    size_t first_tomb = (size_t)-1;
    while (zap_reg[i] != 0) {
        if (zap_reg[i] == base) {
            return;
        }
        if (zap_reg[i] == 1 && first_tomb == (size_t)-1) {
            first_tomb = i;
        }
        i = (i + 1) & mask;
    }
    if (first_tomb != (size_t)-1) {
        i = first_tomb;
    }
    zap_reg[i] = base;
    zap_reg_len++;
}

static void zap_reg_remove(uintptr_t base) {
    if (!zap_reg_cap) {
        return;
    }
    size_t mask = zap_reg_cap - 1;
    size_t i = (base >> 15) & mask;
    while (zap_reg[i] != 0) {
        if (zap_reg[i] == base) {
            zap_reg[i] = 1; /* tombstone */
            zap_reg_len--;
            return;
        }
        i = (i + 1) & mask;
    }
}

static int zap_reg_has(uintptr_t base) {
    if (!zap_reg_cap) {
        return 0;
    }
    size_t mask = zap_reg_cap - 1;
    size_t i = (base >> 15) & mask;
    while (zap_reg[i] != 0) {
        if (zap_reg[i] == base) {
            return 1;
        }
        i = (i + 1) & mask;
    }
    return 0;
}

/* Is `p` a valid GC heap-object payload pointer? */
static int zap_is_heap_ptr(void *p) {
    uintptr_t u = (uintptr_t)p;
    if (u == 0 || (u & 15u) != 0) {
        return 0; /* payloads are 16-byte aligned (header is 16 bytes) */
    }
    uintptr_t base = (u - ZAP_HEADER_BYTES) & ~(uintptr_t)(ZAP_BLOCK - 1);
    if (!zap_reg_has(base)) {
        return 0;
    }
    /* Within the block's usable region (past the reserved metadata lines). */
    return (u - ZAP_HEADER_BYTES) - base >= ZAP_RESERVED_BYTES;
}
static size_t zap_gc_threshold = 4u * 1024u * 1024u; /* initial trigger (4 MiB)   */
static size_t zap_heap_limit = 0;           /* hard cap (ZAP_GC_HEAP_LIMIT), 0=off */
static size_t zap_total_mapped = 0;
static int64_t zap_gc_cycles = 0;           /* collections completed (ticket #77) */

/* Read the optional hard heap cap (ZAP_GC_HEAP_LIMIT, in bytes) once at startup;
 * 0 (the default) disables it. Used by the GC reclamation tests. */
__attribute__((constructor)) static void zap_gc_read_env(void) {
    const char *e = getenv("ZAP_GC_HEAP_LIMIT");
    if (e) {
        long long v = atoll(e);
        if (v > 0) {
            zap_heap_limit = (size_t)v;
        }
    }
    /* ZAP_GC_NO_EVAC disables opportunistic evacuation/defrag (ticket #120),
     * leaving the collector purely mark-in-place. Used to A/B evacuation and as a
     * safety escape hatch; correctness must be identical either way. */
    const char *ne = getenv("ZAP_GC_NO_EVAC");
    if (ne && ne[0] && ne[0] != '0') {
        zap_evac_disabled = 1;
    }
}

/* main-thread shadow-stack head (the main OS thread is a mutator but not a G). */
void *zap_main_shadow_head = NULL;
/* Per-thread "currently active" shadow head, maintained by compiled code. */
__thread void *zap_shadow_head = NULL;

/* A shadow-stack frame, emitted by codegen in each function that holds live GC
 * pointers. Layout MUST match the codegen emission in lib.rs. */
typedef struct ZapShadowFrame {
    struct ZapShadowFrame *prev;
    int64_t nroots;
    void *roots[]; /* nroots slots */
} ZapShadowFrame;

/* ---- global roots (runtime static tables that hold GC pointers) ----------- */
static pthread_mutex_t zap_groots_mu = PTHREAD_MUTEX_INITIALIZER;
static void ***zap_groots = NULL; /* array of addresses of root slots           */
static int64_t zap_groots_len = 0;
static int64_t zap_groots_cap = 0;

/* Register the ADDRESS of a static slot that holds a GC pointer, so the runtime
 * keeps its target alive (e.g. the Cancelled sentinel, interned descriptors are
 * static data and need no registration). Idempotent is not required. */
void zap_gc_add_root(void **slot) {
    pthread_mutex_lock(&zap_groots_mu);
    if (zap_groots_len == zap_groots_cap) {
        int64_t ncap = zap_groots_cap ? zap_groots_cap * 2 : 16;
        void ***n = (void ***)malloc((size_t)ncap * sizeof(void **));
        if (zap_groots) {
            memcpy(n, zap_groots, (size_t)zap_groots_len * sizeof(void **));
            free(zap_groots);
        }
        zap_groots = n;
        zap_groots_cap = ncap;
    }
    zap_groots[zap_groots_len++] = slot;
    pthread_mutex_unlock(&zap_groots_mu);
}

/* ---- cooperative stop-the-world handshake --------------------------------- */
/* Each OS thread that runs Zap code registers a mutator node. A thread is
 * ACTIVE while executing compiled Zap (it must poll zap_safepoint), or PARKED at
 * a safepoint, or BLOCKED inside the runtime (roots already flushed, so the
 * collector may run without waiting for it). */
enum { ZAP_MUT_ACTIVE = 0, ZAP_MUT_PARKED = 1, ZAP_MUT_BLOCKED = 2 };

typedef struct ZapMutator {
    struct ZapMutator *next;
    int state;
    /* Per-P (per-mutator) lock-free allocation buffer (ticket #119). M:P is 1:1
     * and P is pinned to its M, so one buffer per mutator thread == one per P
     * (plus one for the main/aux threads that have no M). `block` is the Immix
     * line allocator's current block (hole-walked, recyclable); `overflow` is a
     * fresh block used to bump medium objects that do not fit `block`'s current
     * hole. Hole/bump state lives in each ZapBlock (single owner at a time), so
     * the common-case allocation is a pointer bump with NO lock; only refilling
     * an exhausted buffer takes zap_heap_mu. Both are reset to NULL by the
     * collector at stop-the-world (see zap_reset_alloc_bufs). */
    ZapBlock *block;
    ZapBlock *overflow;
} ZapMutator;

static pthread_mutex_t zap_stw_mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t zap_stw_cv = PTHREAD_COND_INITIALIZER;
static ZapMutator *zap_mutators = NULL;
static volatile int zap_gc_wanted = 0;      /* a collection is due (set by alloc)  */
static volatile int zap_collecting = 0;     /* collection in progress             */
static __thread ZapMutator *zap_self = NULL;

static void zap_collect_now(void); /* fwd: drive one full collection (caller is
                                    * registered, ACTIVE, about to become the
                                    * collector; it flushes its own head) */

/* Flush this thread's live shadow-stack head into the structure the collector
 * reads: the running G (worker thread) or the main-thread slot. Called with no
 * lock held; the collector only reads these once every mutator has parked. */
static void zap_flush_shadow(void) {
    M *m = cur_m;
    if (m) {
        /* A worker thread. If it is running a G, that G's live head is in the
         * per-thread cell; flush it. If it is idle in the scheduler loop
         * (m->current == NULL), it holds no Zap roots — its last G's head was
         * already saved by switch_to_g — so there is nothing to flush. */
        if (m->current) {
            m->current->shadow_head = zap_shadow_head;
        }
    } else {
        /* The main OS thread runs top-level Zap and is not a G. */
        zap_main_shadow_head = zap_shadow_head;
    }
}

static void zap_mutator_register(void) {
    if (zap_self) {
        return;
    }
    ZapMutator *mu = (ZapMutator *)malloc(sizeof(ZapMutator));
    mu->state = ZAP_MUT_ACTIVE;
    mu->block = NULL;
    mu->overflow = NULL;
    pthread_mutex_lock(&zap_stw_mu);
    mu->next = zap_mutators;
    zap_mutators = mu;
    zap_self = mu;
    pthread_mutex_unlock(&zap_stw_mu);
}

/* Remove this thread's mutator node from the registry — called when a dedicated
 * LockOSThread M exits (ticket #172). Without this, a collection would wait
 * forever on a dead thread's ACTIVE node. Its Immix blocks (if any) are left on
 * the heap and recovered by the next collection's sweep; the node itself is
 * unlinked under the stop-the-world lock so the collector never walks freed
 * memory. Must run OUTSIDE a collection (the caller is idle/blocked). */
static void zap_mutator_unregister(void) {
    if (!zap_self) {
        return;
    }
    pthread_mutex_lock(&zap_stw_mu);
    /* Wait out any in-flight collection so we never unlink mid-mark. */
    while (zap_collecting) {
        pthread_cond_wait(&zap_stw_cv, &zap_stw_mu);
    }
    ZapMutator **pp = &zap_mutators;
    while (*pp && *pp != zap_self) {
        pp = &(*pp)->next;
    }
    if (*pp == zap_self) {
        *pp = zap_self->next;
    }
    /* Relinquish our bump blocks so the collector can reclaim their lines: a
     * detached block simply becomes sweepable (single-owner hole state lived in
     * the block; dropping the reference is enough — the next sweep recycles it). */
    pthread_cond_broadcast(&zap_stw_cv);
    pthread_mutex_unlock(&zap_stw_mu);
    free(zap_self);
    zap_self = NULL;
}

/* Enter a blocking region of the runtime (channel wait, scheduler idle, join):
 * roots are flushed and the thread is marked safe so a collection need not wait
 * for it. Paired with zap_leave_blocking. */
static void zap_enter_blocking(void) {
    if (!zap_self) {
        return;
    }
    zap_flush_shadow();
    pthread_mutex_lock(&zap_stw_mu);
    zap_self->state = ZAP_MUT_BLOCKED;
    pthread_cond_broadcast(&zap_stw_cv); /* a waiting collector may now proceed */
    pthread_mutex_unlock(&zap_stw_mu);
}

static void zap_leave_blocking(void) {
    if (!zap_self) {
        return;
    }
    pthread_mutex_lock(&zap_stw_mu);
    while (zap_collecting) {
        pthread_cond_wait(&zap_stw_cv, &zap_stw_mu);
    }
    zap_self->state = ZAP_MUT_ACTIVE;
    pthread_mutex_unlock(&zap_stw_mu);
}

/* A GC safepoint, polled by compiled code at function entry and loop back-edges
 * (FORK 4a). This is the ONLY place a collection runs (plus the explicit
 * zap_gc_collect): the runtime's own C code has no shadow frames, so GC must
 * never fire while runtime C holds unrooted temporaries. At a safepoint, all of
 * the current thread's live pointers are in its shadow chain, so it is safe.
 *
 * If a collection is due, the first thread here becomes the collector; any other
 * thread that arrives flushes its head, parks, and waits for the collection to
 * finish. */
void zap_safepoint(void) {
    /* Preemption (ticket #172): a monitor-flagged coroutine yields here, where
     * the shadow chain is consistent so parking is provably GC-safe — the
     * precise-GC safe-point machinery doubling as the async-preemption landing
     * point. Skip while a collection is pending/running: the coroutine will park
     * for the GC below instead, which also frees its M. */
    {
        M *m = cur_m;
        if (m && m->preempt && m->current && !zap_gc_wanted && !zap_collecting) {
            m->preempt = 0;
            gopreempt();
        }
    }
    if (!zap_gc_wanted && !zap_collecting) {
        return; /* fast path: two loads, predicted not-taken */
    }
    if (!zap_self) {
        return; /* a thread that never ran Zap holds no un-flushed roots */
    }
    pthread_mutex_lock(&zap_stw_mu);
    if (zap_gc_wanted && !zap_collecting) {
        pthread_mutex_unlock(&zap_stw_mu);
        zap_collect_now(); /* become the collector */
        return;
    }
    /* Another thread is collecting: flush our head and park until it finishes. */
    zap_flush_shadow();
    zap_self->state = ZAP_MUT_PARKED;
    pthread_cond_broadcast(&zap_stw_cv);
    while (zap_collecting) {
        pthread_cond_wait(&zap_stw_cv, &zap_stw_mu);
    }
    zap_self->state = ZAP_MUT_ACTIVE;
    pthread_mutex_unlock(&zap_stw_mu);
}

/* A public mark entry so runtime static-root callbacks (e.g. the reflection
 * table) can enqueue their pointers during tracing. */
void zap_gc_mark(void *payload); /* fwd (defined with the marker below) */

/* Custom root callbacks: runtime modules with static tables of GC pointers
 * register a function that marks them. Called during root enumeration. */
static void (*zap_root_fns[16])(void);
static int zap_root_fn_n = 0;
void zap_gc_add_root_fn(void (*fn)(void)) {
    pthread_mutex_lock(&zap_groots_mu);
    if (zap_root_fn_n < (int)(sizeof(zap_root_fns) / sizeof(zap_root_fns[0]))) {
        zap_root_fns[zap_root_fn_n++] = fn;
    }
    pthread_mutex_unlock(&zap_groots_mu);
}

/* ---- finalizers (ticket #92) ---------------------------------------------- *
 * Boehm registers a finalizer per object (e.g. the `ZapPy` CPython-interop
 * wrapper DECREFs its owned PyObject when collected). The precise collector
 * mirrors that: an object registers a cleanup callback, and when it becomes
 * unreachable the callback runs once, before the object's space is reclaimed.
 *
 * Semantics (documented in design/0008-precise-gc.md; chosen to match Boehm
 * where the ZapPy path cares, and spelled out where it differs):
 *   - ONE-SHOT. Each registration fires at most once; it is removed from the
 *     registry when it fires. A finalizer that resurrects its object (makes it
 *     reachable again) keeps the object, but will not run again unless the
 *     object is re-registered. (`zap_py_finalize` NULLs its ref, so a repeat
 *     would be a harmless no-op anyway.)
 *   - POST-MARK QUEUE. After the normal mark from the true roots, any registered
 *     object whose mark bit is clear is unreachable; all such objects are queued
 *     and removed from the registry in one pass, THEN marked (with their
 *     transitive referents) so the collector does not reclaim an object — or
 *     anything it points at — before its finalizer has run. A finalizable object
 *     therefore survives the collection that queues it and is reclaimed on a
 *     later cycle, once it is no longer registered.
 *   - FINALIZABLE CYCLES. Because the unreachable-registered SET is determined
 *     before any of them is marked, a cycle of mutually-referencing finalizable
 *     objects is still finalized (each exactly once). This is deliberately
 *     simpler than Boehm, which leaves finalizable cycles uncollected; the ZapPy
 *     wrapper never forms such a cycle (each owns one independent CPython ref),
 *     so the observable behavior matches Boehm for the path this ticket exists
 *     for. No ordering is guaranteed among finalizers run in one cycle.
 *   - Finalizers run AFTER the sweep and after the world resumes (in the
 *     collector thread, before the triggering collect returns), so they execute
 *     in ordinary mutator context — never mid-mark — exactly as Boehm runs them
 *     outside the collection. Py_DECREF (which takes the GIL) is safe there.
 *
 * The registry holds raw payload pointers; it is NOT a root (scanned only after
 * marking). A registered object is marked live every cycle it stays registered
 * (by the roots if reachable, or by the queue pass if not), so it is never swept
 * while registered and the stored pointers never dangle. */
typedef void (*ZapFinalizeFn)(void *obj, void *client_data);

typedef struct ZapFinalizer {
    void *payload;
    ZapFinalizeFn fn;
    void *client_data;
} ZapFinalizer;

static pthread_mutex_t zap_fin_mu = PTHREAD_MUTEX_INITIALIZER;
static ZapFinalizer *zap_fin_reg = NULL; /* registered, still-live objects       */
static size_t zap_fin_len = 0;
static size_t zap_fin_cap = 0;
static ZapFinalizer *zap_fin_pending = NULL; /* queued this cycle, run post-sweep */
static size_t zap_fin_pending_len = 0;
static size_t zap_fin_pending_cap = 0;

/* Register `fn(payload, client_data)` to run when `payload` becomes unreachable.
 * Backend-uniform entry (Boehm forwards to GC_register_finalizer in zaprt.c); the
 * ZapPy wrapper is the first user. Idempotent registration is not required. */
void zap_gc_register_finalizer(void *payload, ZapFinalizeFn fn, void *client_data) {
    if (payload == NULL || fn == NULL) {
        return;
    }
    pthread_mutex_lock(&zap_fin_mu);
    if (zap_fin_len == zap_fin_cap) {
        size_t ncap = zap_fin_cap ? zap_fin_cap * 2 : 64;
        zap_fin_reg = (ZapFinalizer *)realloc(zap_fin_reg, ncap * sizeof(ZapFinalizer));
        zap_fin_cap = ncap;
    }
    zap_fin_reg[zap_fin_len].payload = payload;
    zap_fin_reg[zap_fin_len].fn = fn;
    zap_fin_reg[zap_fin_len].client_data = client_data;
    zap_fin_len++;
    pthread_mutex_unlock(&zap_fin_mu);
}

/* ---- the marker ----------------------------------------------------------- */
/* An append-only mark list (BFS worklist) avoids deep C recursion AND lets us
 * clear every survivor's header mark bit after the trace: once the trace drains
 * (cursor == len), list[0..len) is exactly the set of live objects, so we walk
 * it once to reset the header bits for the next cycle. The header mark bit is
 * used only to dedupe this worklist within one collection. */
static void **zap_mark_list = NULL;
static size_t zap_mark_len = 0;
static size_t zap_mark_cursor = 0;
static size_t zap_mark_cap = 0;

static void zap_mark_block_lines(void *payload, size_t obj_size) {
    uintptr_t base = (uintptr_t)payload - ZAP_HEADER_BYTES;
    ZapBlock *b = (ZapBlock *)(base & ~(uintptr_t)(ZAP_BLOCK - 1));
    if (b->kind == ZAP_BK_LOS) {
        b->los_mark = 1;
        return;
    }
    uint32_t first = (uint32_t)((base - (uintptr_t)b) / ZAP_LINE);
    uintptr_t end = base + obj_size - 1;
    uint32_t last = (uint32_t)((end - (uintptr_t)b) / ZAP_LINE);
    if (last >= ZAP_LINES) {
        last = ZAP_LINES - 1;
    }
    for (uint32_t i = first; i <= last; i++) {
        b->line_mark[i] = 1;
    }
}

/* Append a (final-address) live object to the BFS worklist so its children are
 * traced. The caller has already set the mark bit and the line marks. */
static void zap_mark_enqueue(void *payload) {
    if (zap_mark_len == zap_mark_cap) {
        size_t ncap = zap_mark_cap ? zap_mark_cap * 2 : 1024;
        zap_mark_list = (void **)realloc(zap_mark_list, ncap * sizeof(void *));
        zap_mark_cap = ncap;
    }
    zap_mark_list[zap_mark_len++] = payload;
}

/* The small block that contains `payload` (its 32 KiB-aligned base). */
static ZapBlock *zap_block_of(void *payload) {
    uintptr_t base = (uintptr_t)payload - ZAP_HEADER_BYTES;
    return (ZapBlock *)(base & ~(uintptr_t)(ZAP_BLOCK - 1));
}

/* Mark `payload` live IN PLACE (never moving it), mark its lines and enqueue it.
 *
 * Reject null and any pointer that is not one of our heap object payloads.
 * Codegen roots every pointer-TYPED value, some of which are stack allocas,
 * function addresses or inline value-struct pointers — never GC objects — and a
 * precise descriptor field could likewise hold such a pointer; the registry check
 * safely ignores all of them.
 *
 * This is the PINNING primitive (ticket #120): an object marked here before any
 * evacuation runs can never be moved (the moving path below skips an already
 * marked object), so it is how the collector keeps objects reached by a non-slot
 * root — the reflection and signal tables, via the public `zap_gc_mark` — and the
 * runtime-owned buffers of concurrency objects fixed in place. Also used for the
 * finalizer queue, whose registered pointers must stay valid. */
static void zap_mark_push(void *payload) {
    if (!zap_is_heap_ptr(payload)) {
        return;
    }
    if (ZAP_IS_FWD(payload)) {
        /* Defensive: an in-place caller normally never sees a forwarded (moved)
         * object — the pin pass runs before any evacuation and finalizer objects
         * never move — but if one slips through, follow it to the live copy rather
         * than resurrecting the dead stub. */
        payload = ZAP_FWD_TARGET(payload);
    }
    if (ZAP_MARKED(payload)) {
        return;
    }
    const ZapTypeDesc *d = ZAP_DESC(payload);
    if (d == NULL) {
        return;
    }
    (void)d;
    ZAP_SET_MARK(payload);
    zap_mark_block_lines(payload, ZAP_HEADER_BYTES + ZAP_OSIZE(payload));
    zap_mark_enqueue(payload);
}

/* Copy a movable object out of its sparse source block into the dense to-space,
 * install forwarding on the original, and return the new payload — or NULL if
 * to-space cannot be grown (out of blocks), so the caller marks in place instead.
 * The header (incl. the descriptor and the payload size) is memcpy'd first, so
 * the new object is self-describing; the forwarding address is then written over
 * the ORIGINAL's now-dead size slot. The original's lines are left unmarked so
 * the sweep reclaims them. Caller holds zap_heap_mu (the collection does). */
static void *zap_evac_copy(void *payload) {
    size_t osize = ZAP_HEADER_BYTES + ZAP_OSIZE(payload); /* header + live payload */
    /* Advance the dense cursor by the 16-byte-aligned footprint (matching the
     * allocator's zap_round16) so every copied payload stays 16-byte aligned. */
    size_t step = (osize + 15u) & ~(size_t)15u;
    if (!zap_evac_tospace || zap_evac_cursor + step > ZAP_BLOCK) {
        ZapBlock *nb;
        if (zap_free_blocks) {
            nb = zap_free_blocks;
            zap_free_blocks = nb->next;
        } else {
            nb = zap_map_block(); /* aborts on OOM; never NULL */
        }
        nb->evac_src = 0; /* a target is never also a source */
        zap_evac_tospace = nb;
        zap_evac_cursor = ZAP_RESERVED_BYTES;
        if (zap_evac_cursor + step > ZAP_BLOCK) {
            return NULL; /* object larger than a block's usable region (unreachable) */
        }
    }
    void *old_hdr = (char *)payload - ZAP_HEADER_BYTES;
    void *new_hdr = (char *)zap_evac_tospace + zap_evac_cursor;
    memcpy(new_hdr, old_hdr, osize);
    zap_evac_cursor += (uint32_t)step;
    void *np = (char *)new_hdr + ZAP_HEADER_BYTES;
    ZAP_SET_FWD(payload, np); /* original is now a forwarding stub (dead) */
    zap_evac_moved++;
    return np;
}

/* Process a reference VALUE for the (possibly moving) trace. Returns the address
 * the reference should now hold:
 *   - a non-heap pointer / null: unchanged;
 *   - an already-forwarded object: its new address (the move happened earlier);
 *   - an already-live object: unchanged;
 *   - otherwise mark it: evacuate it (when enabled and it is a movable leaf buffer
 *     in a sparse source block) to a new address, else mark it in place.
 * Either way the (final) object is enqueued once for child tracing. */
static void *zap_evac_mark(void *payload) {
    if (!zap_is_heap_ptr(payload)) {
        return payload;
    }
    if (ZAP_IS_FWD(payload)) {
        return ZAP_FWD_TARGET(payload);
    }
    if (ZAP_MARKED(payload)) {
        return payload;
    }
    const ZapTypeDesc *d = ZAP_DESC(payload);
    if (d == NULL) {
        return payload;
    }
    void *final = payload;
    if (zap_evac_active && d == &zap_desc_atomic) {
        ZapBlock *b = zap_block_of(payload);
        if (b->kind == ZAP_BK_SMALL && b->evac_src) {
            void *np = zap_evac_copy(payload);
            if (np) {
                final = np;
            }
        }
    }
    ZAP_SET_MARK(final);
    zap_mark_block_lines(final, ZAP_HEADER_BYTES + ZAP_OSIZE(final));
    zap_mark_enqueue(final);
    return final;
}

/* Trace a genuine pointer-typed slot, rewriting it in place if its target moved. */
static void zap_process_slot(void **slot) {
    void *p = *slot;
    void *np = zap_evac_mark(p);
    if (np != p) {
        *slot = np;
    }
}

/* Trace a container interior word that holds pointer bits (list/dict/set/heap
 * element words stored as int64), rewriting it in place if its target moved. */
static void zap_process_word(int64_t *slot) {
    void *p = (void *)(intptr_t)*slot;
    void *np = zap_evac_mark(p);
    if (np != p) {
        *slot = (int64_t)(intptr_t)np;
    }
}

/* Trace one already-live object: process each of its references for the (moving)
 * collector. A genuine pointer field or buffer-pointer slot goes through
 * `zap_process_slot` so it is rewritten in place if its target was evacuated; a
 * container interior word (a pointer stored as int64) goes through
 * `zap_process_word`. A container's own data/entry buffer is itself a movable
 * `zap_desc_atomic` leaf, so its slot is processed FIRST and then re-read, so the
 * interior is walked at the buffer's (possibly new) address.
 *
 * Two runtime-owned concurrency objects — `ZapChan` and `Scope` — keep their data
 * buffer PINNED (marked in place, not moved): their C routines can hold a raw
 * pointer to that buffer across a blocking park/resume, where a relocation would
 * not be observed. The buffered element slots inside are still processed
 * move-aware (those words are only read with the object's lock held, never cached
 * across a park). The coroutine `G` is likewise scheduler-owned and held by raw C
 * pointers, so its pointer fields are pinned in place. */
static void zap_trace_children(void *payload) {
    const ZapTypeDesc *d = ZAP_DESC(payload);
    switch (d->kind) {
    case ZAP_TD_ATOMIC:
        return;
    case ZAP_TD_PRECISE: {
        char *p = (char *)payload;
        if (d == &zap_desc_gobj) {
            /* Scheduler-owned: pin the G's referents in place (raw C holds them). */
            for (int32_t i = 0; i < d->n_ptrs; i++) {
                zap_mark_push(*(void **)(p + d->ptr_offsets[i]));
            }
        } else {
            for (int32_t i = 0; i < d->n_ptrs; i++) {
                zap_process_slot((void **)(p + d->ptr_offsets[i]));
            }
        }
        return;
    }
    case ZAP_TD_LIST: {
        ZapList *l = (ZapList *)payload;
        if (l->data) {
            zap_process_slot((void **)&l->data);
            if (l->elem_ptr) {
                int64_t *data = l->data;
                for (int64_t i = 0; i < l->len; i++) {
                    zap_process_word(&data[i]);
                }
            }
        }
        return;
    }
    case ZAP_TD_DICT: {
        ZapDict *dd = (ZapDict *)payload;
        if (dd->entries) {
            zap_process_slot((void **)&dd->entries);
            if (dd->key_ptr || dd->val_ptr) {
                ZapEntry *e = dd->entries;
                for (int64_t i = 0; i < dd->cap; i++) {
                    if (e[i].state != SLOT_FULL) {
                        continue;
                    }
                    if (dd->key_ptr) {
                        zap_process_word(&e[i].key);
                    }
                    if (dd->val_ptr) {
                        zap_process_word(&e[i].val);
                    }
                }
            }
        }
        return;
    }
    case ZAP_TD_SET: {
        ZapSet *s = (ZapSet *)payload;
        if (s->entries) {
            zap_process_slot((void **)&s->entries);
            if (s->elem_ptr) {
                ZapSetEntry *e = s->entries;
                for (int64_t i = 0; i < s->cap; i++) {
                    if (e[i].state == SLOT_FULL) {
                        zap_process_word(&e[i].key);
                    }
                }
            }
        }
        return;
    }
    case ZAP_TD_HEAP: {
        ZapHeap *h = (ZapHeap *)payload;
        if (h->data) {
            zap_process_slot((void **)&h->data);
            if (h->kind == HEAP_ELEM_STR) {
                int64_t *data = h->data;
                for (int64_t i = 0; i < h->len; i++) {
                    zap_process_word(&data[i]);
                }
            }
        }
        return;
    }
    case ZAP_TD_CHAN: {
        ZapChan *c = (ZapChan *)payload;
        if (c->buf) {
            zap_mark_push(c->buf); /* pin: raw C may hold it across a park */
            if (c->elem_ptr && c->cap > 0) {
                /* the `count` buffered words live at [head .. head+count) mod cap */
                for (int64_t k = 0; k < c->count; k++) {
                    int64_t idx = (c->head + k) % c->cap;
                    zap_process_word((int64_t *)&c->buf[idx]);
                }
            }
        }
        /* In-flight (unbuffered rendezvous) values sit in a blocked sender's
         * stack-local Waiter, which is rooted via that coroutine's shadow
         * chain, so they need no marking here. */
        return;
    }
    case ZAP_TD_ARRAY: {
        ZapArray *a = (ZapArray *)payload;
        if (a->dims) {
            zap_process_slot((void **)&a->dims); /* int64 dims: atomic buffer */
        }
        if (a->data) {
            zap_process_slot((void **)&a->data); /* numeric data: atomic, no ptrs */
        }
        return;
    }
    case ZAP_TD_MATCH: {
        ZapMatch *mm = (ZapMatch *)payload;
        if (mm->groups) {
            zap_process_slot((void **)&mm->groups);
            ZapStr **groups = mm->groups;
            for (int64_t i = 0; i < mm->ngroups; i++) {
                zap_process_slot((void **)&groups[i]);
            }
        }
        if (mm->names) {
            zap_process_slot((void **)&mm->names);
            ZapStr **names = mm->names;
            for (int64_t i = 0; i < mm->nnames; i++) {
                zap_process_slot((void **)&names[i]);
            }
        }
        if (mm->name_num) {
            zap_process_slot((void **)&mm->name_num); /* int64 buffer: atomic */
        }
        return;
    }
    case ZAP_TD_SCOPE: {
        Scope *sc = (Scope *)payload;
        if (sc->excs) {
            zap_mark_push((void *)sc->excs); /* pin: runtime-owned buffer */
            ZapStr **excs = sc->excs;
            for (int64_t i = 0; i < sc->nexc; i++) {
                zap_process_slot((void **)&excs[i]);
            }
        }
        return;
    }
    case ZAP_TD_GSTACK:
        return; /* a raw stack buffer: its live pointers are reached via the G's
                 * shadow chain, not by scanning the stack bytes. */
    default:
        return;
    }
}

static void zap_mark_drain(void) {
    while (zap_mark_cursor < zap_mark_len) {
        zap_trace_children(zap_mark_list[zap_mark_cursor++]);
    }
}

/* After the trace drains, list[0..len) is exactly the live set; clear each
 * survivor's header mark bit so the next collection starts clean, then reset the
 * worklist. */
static void zap_mark_reset(void) {
    for (size_t i = 0; i < zap_mark_len; i++) {
        ZAP_CLR_MARK(zap_mark_list[i]);
    }
    zap_mark_len = 0;
    zap_mark_cursor = 0;
}

/* Public mark entry for runtime static-root callbacks. */
void zap_gc_mark(void *payload) { zap_mark_push(payload); }

/* After the true-root mark has drained, move every registered object that is now
 * unreachable (mark bit clear) onto the pending-finalizer queue and drop it from
 * the registry (one-shot), THEN mark all of them — so their transitive referents
 * survive this cycle and the finalizer can safely dereference the object. The
 * set is collected before any marking so a finalizable cycle still finalizes.
 * Caller holds zap_heap_mu and the world is stopped; the caller drains the mark
 * worklist again afterward. */
static void zap_finalize_prepare(void) {
    pthread_mutex_lock(&zap_fin_mu);
    size_t first_new = zap_fin_pending_len;
    size_t i = 0;
    while (i < zap_fin_len) {
        if (ZAP_MARKED(zap_fin_reg[i].payload)) {
            i++; /* still reachable: keep registered */
            continue;
        }
        /* Unreachable: queue for finalization and swap-remove from the registry. */
        if (zap_fin_pending_len == zap_fin_pending_cap) {
            size_t ncap = zap_fin_pending_cap ? zap_fin_pending_cap * 2 : 64;
            zap_fin_pending =
                (ZapFinalizer *)realloc(zap_fin_pending, ncap * sizeof(ZapFinalizer));
            zap_fin_pending_cap = ncap;
        }
        zap_fin_pending[zap_fin_pending_len++] = zap_fin_reg[i];
        zap_fin_reg[i] = zap_fin_reg[--zap_fin_len];
        /* do not advance i: the swapped-in entry must be examined */
    }
    /* Keep every queued object (and what it references) alive for this cycle. */
    for (size_t k = first_new; k < zap_fin_pending_len; k++) {
        zap_mark_push(zap_fin_pending[k].payload);
    }
    pthread_mutex_unlock(&zap_fin_mu);
}

/* Run and clear the pending finalizers. Called by the collector thread after the
 * sweep and after the world has resumed, so finalizers run in ordinary mutator
 * context (never mid-mark). The queued objects were marked live this cycle, so
 * their memory is intact; they are reclaimed on a later cycle now that they are
 * unregistered. No safepoint runs between resume and here, so the pointers stay
 * valid even though they are not rooted. */
static void zap_run_finalizers(void) {
    /* Snapshot under the lock, then run without it (a finalizer may re-register,
     * which takes zap_fin_mu). */
    pthread_mutex_lock(&zap_fin_mu);
    size_t n = zap_fin_pending_len;
    ZapFinalizer *batch = NULL;
    if (n) {
        batch = (ZapFinalizer *)malloc(n * sizeof(ZapFinalizer));
        memcpy(batch, zap_fin_pending, n * sizeof(ZapFinalizer));
        zap_fin_pending_len = 0;
    }
    pthread_mutex_unlock(&zap_fin_mu);
    for (size_t i = 0; i < n; i++) {
        batch[i].fn(batch[i].payload, batch[i].client_data);
    }
    free(batch);
}

/* ---- root enumeration ----------------------------------------------------- */
/* A shadow frame's root slots are genuine pointer slots the collector may rewrite
 * when an object moves — and the exact slots compiled code reloads from after a
 * safepoint (load-at-use, #120), so a relocation is observed by the mutator. */
static void zap_mark_shadow_chain(void *head) {
    for (ZapShadowFrame *f = (ZapShadowFrame *)head; f; f = f->prev) {
        for (int64_t i = 0; i < f->nroots; i++) {
            zap_process_slot(&f->roots[i]);
        }
    }
}

static void zap_mark_roots(void) {
    pthread_mutex_lock(&zap_groots_mu);
    /* PIN PASS FIRST (ticket #120): custom root callbacks (the reflection and
     * signal tables) mark their objects via the non-slot `zap_gc_mark`, which
     * cannot report back a new address, so those objects must not move. Running
     * them before any evacuation marks them in place; the moving paths below then
     * skip them (an already-marked object is never relocated). Nothing is
     * forwarded yet, so no stale reference can be created here. */
    for (int i = 0; i < zap_root_fn_n; i++) {
        zap_root_fns[i]();
    }
    /* Global static roots: each entry is the ADDRESS of a slot holding a GC
     * pointer, so process it move-aware to rewrite the slot if its target moves. */
    for (int64_t i = 0; i < zap_groots_len; i++) {
        zap_process_slot(zap_groots[i]);
    }
    pthread_mutex_unlock(&zap_groots_mu);

    /* Main-thread shadow chain (top-level Zap code runs on the main OS thread,
     * which is not a G). Its head was flushed when it parked or blocked. */
    zap_mark_shadow_chain(zap_main_shadow_head);

    /* Every live coroutine: the G object itself (keeps its green stack buffer,
     * closure arg and owning scope alive via zap_desc_gobj) AND its saved shadow
     * chain. A running G's live head was flushed into the G when its M parked at
     * the safepoint; a parked/runnable G's head was saved by switch_to_g when it
     * left its M. g_all registers every live G (keeping parked ones reachable);
     * run-queue linkage needs no separate tracing because every queued G is in
     * g_all too. The G is scheduler-owned (held by raw C pointers), so it is
     * pinned in place; its green stack never moves (its own descriptor, not a
     * movable leaf), so the shadow-frame slots processed above are stable. */
    pthread_mutex_lock(&g_all_mu);
    for (int64_t i = 0; i < g_all_len; i++) {
        G *g = g_all[i];
        if (g) {
            zap_mark_push(g); /* pin the G in place */
            zap_mark_shadow_chain(g->shadow_head);
        }
    }
    pthread_mutex_unlock(&g_all_mu);

    /* Pooled (freed-but-retained) coroutines: a dead G is unlinked from g_all by
     * gfree and parked on the g_free_head free list to recycle its 128 KiB green
     * stack (galloc reuses it rather than remapping). That free list is held only
     * by raw C pointers, so under precise GC it is NOT otherwise a root: without
     * this, the first collection after a G is freed reclaims the G object and
     * unmaps its green-stack buffer, and the next spawn that pops it from the free
     * list faults in makecontext writing the (now unmapped) stack — the gc-immix
     * SIGSEGV under sleep+mutex+mass-spawn in #188 (bug 1). gc-boehm never saw it
     * because it conservatively scans the static g_free_head pointer. Pin each
     * pooled G in place (which keeps its stack/arg/scope referents alive via
     * zap_desc_gobj); a pooled G carries no live frame, so its (stale) shadow_head
     * is deliberately NOT traced. */
    pthread_mutex_lock(&g_free_mu);
    for (G *g = g_free_head; g; g = g->link) {
        zap_mark_push(g);
    }
    pthread_mutex_unlock(&g_free_mu);
}

/* ---- sweep ---------------------------------------------------------------- */
static void zap_block_reset_marks(ZapBlock *b) {
    memset(b->line_mark, 0, sizeof(b->line_mark));
    for (uint32_t i = 0; i < ZAP_RESERVED_LINES; i++) {
        b->line_mark[i] = 1; /* metadata lines are never allocatable */
    }
}

/* Find the next run of free (unmarked) lines in a small block at or after
 * `from_line`; set the block's cursor/limit to it. Returns 1 if a hole was
 * found, 0 if the block is full. */
static int zap_block_next_hole(ZapBlock *b) {
    uint32_t i = b->search_line;
    while (i < ZAP_LINES && b->line_mark[i]) {
        i++;
    }
    if (i >= ZAP_LINES) {
        b->hole_cursor = b->hole_limit = ZAP_BLOCK;
        b->search_line = ZAP_LINES;
        return 0;
    }
    uint32_t start = i;
    while (i < ZAP_LINES && !b->line_mark[i]) {
        i++;
    }
    b->hole_cursor = start * ZAP_LINE;
    b->hole_limit = i * ZAP_LINE;
    b->search_line = i;
    return 1;
}

static void zap_sweep(void) {
    zap_free_blocks = NULL;
    zap_recycle_blocks = NULL;
    size_t live = 0;
    ZapBlock **pp = &zap_all_blocks;
    while (*pp) {
        ZapBlock *b = *pp;
        if (b->kind == ZAP_BK_LOS) {
            if (b->los_mark) {
                b->los_mark = 0;
                live += b->los_size;
                pp = &b->all_next;
            } else {
                *pp = b->all_next;
                zap_reg_remove((uintptr_t)b);
                zap_total_mapped -= ((b->los_size + sizeof(ZapBlock) + ZAP_BLOCK - 1) /
                                     ZAP_BLOCK) *
                                    ZAP_BLOCK;
                free(b);
            }
            continue;
        }
        /* Small block: count live lines. Record the occupancy so the NEXT
         * collection can pick the sparsest blocks as evacuation sources (#120). */
        uint32_t used = 0;
        for (uint32_t i = ZAP_RESERVED_LINES; i < ZAP_LINES; i++) {
            if (b->line_mark[i]) {
                used++;
            }
        }
        b->live_lines = used;
        b->evac_src = 0;
        live += (size_t)used * ZAP_LINE;
        if (used == 0) {
            zap_block_reset_marks(b);
            b->search_line = ZAP_RESERVED_LINES;
            zap_block_next_hole(b);
            b->avail = (ZAP_LINES - ZAP_RESERVED_LINES) * ZAP_LINE;
            b->next = zap_free_blocks;
            zap_free_blocks = b;
        } else {
            b->search_line = ZAP_RESERVED_LINES;
            if (zap_block_next_hole(b)) {
                /* Free (unmarked, non-reserved) lines available for reuse. */
                b->avail = (ZAP_LINES - ZAP_RESERVED_LINES - used) * ZAP_LINE;
                b->next = zap_recycle_blocks;
                zap_recycle_blocks = b;
            }
            /* else fully used: not placed on any alloc list this cycle. */
        }
        pp = &b->all_next;
    }
    zap_live_estimate = live;
}

/* ---- the collection ------------------------------------------------------- */
static void zap_mark_sweep(void); /* fwd: the mark + sweep under zap_heap_mu */

/* Drop every mutator's per-P allocation buffers at stop-the-world (ticket #119).
 * Called by the collector with the world stopped and zap_stw_mu held, so the
 * mutator list is stable and no thread is mid-allocation (allocation is never a
 * safepoint, so a thread that reached the handshake is PARKED/BLOCKED outside
 * the allocator). The blocks the buffers referenced stay in zap_all_blocks and
 * are swept normally: their live lines are marked by the trace, and the free
 * tail past each bump cursor becomes a recyclable hole. Each mutator re-reads
 * block/overflow only after re-acquiring zap_stw_mu on wake (a happens-before
 * edge), so it observes NULL and refills a fresh block — correctly handling a
 * coroutine that migrated Ps, a mutator that held no block, and the refill/
 * collect race (serialized because refill keeps the thread ACTIVE, which the
 * handshake above waits out). */
static void zap_reset_alloc_bufs(void) {
    for (ZapMutator *m = zap_mutators; m; m = m->next) {
        m->block = NULL;
        m->overflow = NULL;
    }
}

/* Drive one full stop-the-world collection. The caller is a registered mutator
 * executing compiled code at a safepoint (so its own live pointers are already
 * in its shadow chain). Only one thread collects at a time; others park in
 * zap_safepoint / zap_*_blocking. */
static void zap_collect_now(void) {
    pthread_mutex_lock(&zap_stw_mu);
    if (zap_collecting) {
        /* Lost the race: another thread already collects. Park and wait. */
        zap_flush_shadow();
        if (zap_self) {
            zap_self->state = ZAP_MUT_PARKED;
            pthread_cond_broadcast(&zap_stw_cv);
        }
        while (zap_collecting) {
            pthread_cond_wait(&zap_stw_cv, &zap_stw_mu);
        }
        if (zap_self) {
            zap_self->state = ZAP_MUT_ACTIVE;
        }
        pthread_mutex_unlock(&zap_stw_mu);
        return;
    }
    zap_collecting = 1;
    zap_gc_wanted = 0;
    zap_flush_shadow();
    if (zap_self) {
        zap_self->state = ZAP_MUT_PARKED;
    }
    /* Wait until no OTHER mutator is ACTIVE (each flushes its head as it parks
     * or blocks). BLOCKED threads are already safe. */
    for (;;) {
        int active = 0;
        for (ZapMutator *m = zap_mutators; m; m = m->next) {
            if (m != zap_self && m->state == ZAP_MUT_ACTIVE) {
                active++;
            }
        }
        if (active == 0) {
            break;
        }
        pthread_cond_wait(&zap_stw_cv, &zap_stw_mu);
    }
    /* World stopped and the mutator list is stable under zap_stw_mu: relinquish
     * every P-local allocation buffer so the sweep can reclaim their blocks and
     * each mutator refills fresh on resume (ticket #119). */
    zap_reset_alloc_bufs();
    pthread_mutex_unlock(&zap_stw_mu);

    /* World is stopped: every live pointer is reachable from a flushed shadow
     * chain, a G, or a global root. Mark and sweep under the heap lock. */
    pthread_mutex_lock(&zap_heap_mu);
    zap_mark_sweep();
    zap_gc_cycles++;
    pthread_mutex_unlock(&zap_heap_mu);

    pthread_mutex_lock(&zap_stw_mu);
    zap_collecting = 0;
    if (zap_self) {
        zap_self->state = ZAP_MUT_ACTIVE;
    }
    pthread_cond_broadcast(&zap_stw_cv);
    pthread_mutex_unlock(&zap_stw_mu);

    /* World resumed: run any finalizers queued during this collection in ordinary
     * mutator context (ticket #92). Objects queued this cycle were marked live, so
     * their memory is intact until a later collection reclaims them. */
    zap_run_finalizers();
}

/* Explicit collection (a `gc_collect()` builtin / test hook, and the uniform
 * ticket-#77 entry). Safe to call from compiled Zap code, where the caller's
 * roots are shadow-flushed. */
void zap_gc_collect(void) {
    zap_mutator_register();
    zap_collect_now();
}

/* Number of collections run so far (uniform ticket-#77 entry; Boehm uses
 * GC_get_gc_no). `zap_gc_cycles` is bumped under zap_heap_mu inside
 * zap_collect_now, so it covers both safepoint-driven and explicit cycles. */
int64_t zap_gc_count(void) {
    pthread_mutex_lock(&zap_heap_mu);
    int64_t n = zap_gc_cycles;
    pthread_mutex_unlock(&zap_heap_mu);
    return n;
}

/* Uniform GC stats (ticket #77): mapped-heap bytes and the live bytes computed
 * at the last collection. Backend-agnostic signature matching the Boehm one. */
void zap_gc_stats(int64_t *heap_bytes, int64_t *live_bytes) {
    pthread_mutex_lock(&zap_heap_mu);
    size_t mapped = zap_total_mapped;
    size_t live = zap_live_estimate;
    pthread_mutex_unlock(&zap_heap_mu);
    if (heap_bytes) {
        *heap_bytes = (int64_t)mapped;
    }
    if (live_bytes) {
        *live_bytes = (int64_t)live;
    }
}

/* ---- block / object allocation -------------------------------------------- */
static ZapBlock *zap_map_block(void) {
    void *mem = NULL;
    if (posix_memalign(&mem, ZAP_BLOCK, ZAP_BLOCK) != 0 || !mem) {
        fprintf(stderr, "zap: out of memory (block map)\n");
        abort();
    }
    ZapBlock *b = (ZapBlock *)mem;
    b->kind = ZAP_BK_SMALL;
    b->all_next = zap_all_blocks;
    zap_all_blocks = b;
    zap_reg_insert((uintptr_t)b);
    zap_total_mapped += ZAP_BLOCK;
    zap_block_reset_marks(b);
    b->search_line = ZAP_RESERVED_LINES;
    zap_block_next_hole(b);
    b->avail = (ZAP_LINES - ZAP_RESERVED_LINES) * ZAP_LINE;
    b->live_lines = 0; /* fresh: not an evacuation candidate until a sweep (#120) */
    b->evac_src = 0;
    return b;
}

/* ---- GC-trigger / cap accounting (ticket #119) ---------------------------- *
 * With per-P bump buffers the fast path takes no lock, so the allocated-bytes
 * counter is no longer bumped per object. Instead it is advanced per block at
 * refill (the slow path, under zap_heap_mu) by that block's free capacity
 * (`b->avail`, recorded when the block was placed on a list / mapped). A mutator
 * consumes essentially all of a line block's holes before it refills again, so
 * block-granularity accounting tracks real allocation closely; it can only
 * slightly OVER-count (a medium object abandoning an overflow block's tail),
 * which makes the collector fire a touch early — the safe direction for the
 * ZAP_GC_HEAP_LIMIT cap. Caller holds zap_heap_mu. */
static void zap_account_refill(ZapBlock *b) {
    zap_bytes_since_gc += b->avail;
    if (zap_bytes_since_gc >= zap_gc_threshold) {
        zap_gc_wanted = 1; /* collect at the next safepoint */
    }
    /* Enforce the optional hard cap (ZAP_GC_HEAP_LIMIT) on mapped memory. Freed
     * blocks are reused rather than unmapped, so with a working collector the
     * mapped peak plateaus at roughly live + one threshold (well under a cap set
     * generously above the threshold); exceeding it means the collector is not
     * reclaiming (or a safepoint is never reached), so abort. We cannot collect
     * here (runtime C holds unrooted temporaries), but aborting is always safe.
     * Used only by the GC reclamation tests; the cap is unset in normal runs. */
    if (zap_heap_limit && zap_total_mapped > zap_heap_limit) {
        fprintf(stderr, "zap: mapped heap exceeds ZAP_GC_HEAP_LIMIT %zu\n", zap_heap_limit);
        abort();
    }
}

/* Refill a mutator's LINE allocator: a block that may still hold free holes is
 * preferred (recycle), then a fully-free block, then a fresh map. Returns a
 * block with its first hole set up. Slow path; takes zap_heap_mu. */
static ZapBlock *zap_refill_line(void) {
    pthread_mutex_lock(&zap_heap_mu);
    ZapBlock *b;
    if (zap_recycle_blocks) {
        b = zap_recycle_blocks;
        zap_recycle_blocks = b->next;
    } else if (zap_free_blocks) {
        b = zap_free_blocks;
        zap_free_blocks = b->next;
    } else {
        b = zap_map_block();
    }
    zap_account_refill(b);
    pthread_mutex_unlock(&zap_heap_mu);
    return b;
}

/* Refill a mutator's OVERFLOW allocator (medium objects): only a fully-empty
 * block will do, so the block always offers one full-block hole big enough for
 * any medium object (<= ZAP_LOS_THRESHOLD). Recycle blocks are deliberately not
 * used here — their fragmented holes would defeat the overflow allocator. Slow
 * path; takes zap_heap_mu. */
static ZapBlock *zap_refill_overflow(void) {
    pthread_mutex_lock(&zap_heap_mu);
    ZapBlock *b;
    if (zap_free_blocks) {
        b = zap_free_blocks;
        zap_free_blocks = b->next;
    } else {
        b = zap_map_block();
    }
    zap_account_refill(b);
    pthread_mutex_unlock(&zap_heap_mu);
    return b;
}

/* Fast-path LINE allocation for small objects (osize <= ZAP_LINE) out of the
 * current mutator's own block. Lock-free: a bump + limit check within an owned
 * block (single owner at a time). Only an exhausted buffer takes the lock
 * (zap_refill_line). */
static void *zap_bump_line(ZapMutator *self, size_t osize) {
    for (;;) {
        ZapBlock *b = self->block;
        if (b) {
            if (b->hole_cursor + osize <= b->hole_limit) {
                void *p = (char *)b + b->hole_cursor;
                b->hole_cursor += (uint32_t)osize;
                return p;
            }
            /* current hole too small: walk to the next hole in this block */
            if (b->hole_limit < ZAP_BLOCK && zap_block_next_hole(b)) {
                continue;
            }
        }
        /* no block, or this block is exhausted: refill under the lock */
        self->block = zap_refill_line();
    }
}

/* Fast-path allocation for medium objects (ZAP_LINE < osize <= LOS threshold).
 * Prefer the line block's current hole when the object fits it (density), else
 * bump the overflow block (fresh blocks, so any medium fits). Lock-free except
 * for overflow refill. */
static void *zap_bump_medium(ZapMutator *self, size_t osize) {
    ZapBlock *b = self->block;
    if (b && b->hole_cursor + osize <= b->hole_limit) {
        void *p = (char *)b + b->hole_cursor;
        b->hole_cursor += (uint32_t)osize;
        return p;
    }
    for (;;) {
        ZapBlock *o = self->overflow;
        if (o && o->hole_cursor + osize <= o->hole_limit) {
            void *p = (char *)o + o->hole_cursor;
            o->hole_cursor += (uint32_t)osize;
            return p;
        }
        self->overflow = zap_refill_overflow();
    }
}

/* Allocate `osize` (incl. header) bytes of small-object space from the current
 * mutator's lock-free per-P buffer; returns the header address. */
static void *zap_bump_mutator(size_t osize) {
    ZapMutator *self = zap_self;
    if (!self) {
        zap_mutator_register();
        self = zap_self;
    }
    if (osize <= ZAP_LINE) {
        return zap_bump_line(self, osize);
    }
    return zap_bump_medium(self, osize);
}

/* Large-object allocation (osize > ZAP_LOS_THRESHOLD): a dedicated mapping, one
 * object per block. Rare and already costly, so it keeps the global lock rather
 * than a per-P buffer. Takes zap_heap_mu (shared with the block lists, the
 * registry and the collector's sweep). */
static void *zap_alloc_los(size_t osize) {
    size_t total = ((osize + sizeof(ZapBlock) + ZAP_BLOCK - 1) / ZAP_BLOCK) * ZAP_BLOCK;
    void *mem = NULL;
    if (posix_memalign(&mem, ZAP_BLOCK, total) != 0 || !mem) {
        fprintf(stderr, "zap: out of memory (LOS map)\n");
        abort();
    }
    pthread_mutex_lock(&zap_heap_mu);
    ZapBlock *b = (ZapBlock *)mem;
    b->kind = ZAP_BK_LOS;
    b->los_mark = 0;
    b->los_size = osize;
    b->all_next = zap_all_blocks;
    zap_all_blocks = b;
    zap_reg_insert((uintptr_t)b);
    zap_total_mapped += total;
    zap_bytes_since_gc += osize;
    if (zap_bytes_since_gc >= zap_gc_threshold) {
        zap_gc_wanted = 1; /* collect at the next safepoint */
    }
    if (zap_heap_limit && zap_total_mapped > zap_heap_limit) {
        pthread_mutex_unlock(&zap_heap_mu);
        fprintf(stderr, "zap: mapped heap exceeds ZAP_GC_HEAP_LIMIT %zu\n", zap_heap_limit);
        abort();
    }
    pthread_mutex_unlock(&zap_heap_mu);
    /* Object header placed right after the block metadata; a block-aligned
     * lookup via (payload-16) & ~(BLOCK-1) lands on `b` (the payload sits in the
     * first 32 KiB region). */
    return (char *)b + ZAP_RESERVED_BYTES;
}

/* Round an object size up to 16 bytes so every payload stays 16-byte aligned
 * (the block base and reserved region are already 128-byte aligned). */
static size_t zap_round16(size_t n) { return (n + 15u) & ~(size_t)15u; }

/* Core allocation: `payload_size` bytes of payload for descriptor `desc`.
 * Returns a zeroed payload pointer with the header initialized. Allocation NEVER
 * collects — it only flags that a collection is due; the next GC safepoint in
 * compiled code (where all live pointers are shadow-rooted) runs it. This is
 * what makes a C runtime without shadow frames safe: GC cannot fire while the
 * runtime holds unrooted temporaries, because the runtime never polls a
 * safepoint mid-operation. */
static void *zap_gc_alloc_sized(const ZapTypeDesc *desc, size_t payload_size) {
    size_t osize = zap_round16(ZAP_HEADER_BYTES + payload_size);
    void *hdr;
    if (ZAP_HEADER_BYTES + payload_size > ZAP_LOS_THRESHOLD) {
        hdr = zap_alloc_los(osize); /* large objects: global lock (rare) */
    } else {
        hdr = zap_bump_mutator(osize); /* small/medium: lock-free per-P buffer */
    }
    void *payload = (char *)hdr + ZAP_HEADER_BYTES;
    ZAP_HDR(payload) = (uintptr_t)desc; /* mark bit 0 clear */
    ZAP_OSIZE(payload) = payload_size;
    memset(payload, 0, payload_size);
    return payload;
}

void *zap_gc_alloc(const ZapTypeDesc *desc) {
    return zap_gc_alloc_sized(desc, (size_t)desc->size);
}

/* A leaf/atomic buffer of `size` bytes (string data, numeric array data,
 * container element/entry buffers whose interiors the container traces). */
void *zap_gc_alloc_atomic(int64_t size) {
    return zap_gc_alloc_sized(&zap_desc_atomic, (size_t)(size < 0 ? 0 : size));
}

/* Variable-length object (descriptor is a container kind, but the payload size
 * is runtime-determined — used for the array dims/strides buffers via atomic;
 * kept for symmetry). */
void *zap_gc_alloc_var(const ZapTypeDesc *desc, int64_t size) {
    return zap_gc_alloc_sized(desc, (size_t)(size < 0 ? 0 : size));
}

/* Mark + sweep. Caller holds zap_heap_mu and the world is stopped. */
static void zap_mark_sweep(void) {
    /* Clear line / LOS marks (object header mark bits are already clear: every
     * survivor of the previous cycle had its bit reset by zap_mark_reset, and
     * fresh allocations are memset to a clear header). In the same pass, select
     * evacuation SOURCE blocks (ticket #120): a small block that was non-empty but
     * at most half-occupied at the previous sweep is fragmented, so relocating its
     * few survivors frees the whole block. The occupancy is last cycle's
     * (`live_lines`, set by zap_sweep) because this cycle's marks are not in yet;
     * the count is capped so copy work and to-space stay bounded. */
    int src = 0;
    for (ZapBlock *b = zap_all_blocks; b; b = b->all_next) {
        if (b->kind == ZAP_BK_SMALL) {
            memset(b->line_mark, 0, sizeof(b->line_mark));
            for (uint32_t i = 0; i < ZAP_RESERVED_LINES; i++) {
                b->line_mark[i] = 1;
            }
            b->evac_src = 0;
            if (!zap_evac_disabled && b->live_lines > 0 &&
                b->live_lines <= ZAP_EVAC_SPARSE_LINES && src < (int)ZAP_EVAC_MAX_SRC) {
                b->evac_src = 1;
                src++;
            }
        } else {
            b->los_mark = 0;
        }
    }
    zap_evac_active = (src > 0);
    zap_evac_tospace = NULL;
    zap_evac_cursor = 0;
    zap_evac_moved = 0;
    zap_mark_roots();
    zap_mark_drain();
    zap_finalize_prepare(); /* queue now-unreachable finalizable objects (#92) */
    zap_mark_drain();       /* keep queued objects + their referents alive       */
    zap_mark_reset();       /* clear survivor header bits for the next cycle      */
    zap_sweep();
    zap_bytes_since_gc = 0;
    /* Optional evacuation visibility (ticket #120): one line per collection naming
     * how many source blocks were selected and how many objects were relocated.
     * Off unless ZAP_GC_EVAC_STATS is set, so normal runs are unaffected; used to
     * confirm evacuation fires and, later, to measure it from zap-perf. */
    if (zap_evac_active && getenv("ZAP_GC_EVAC_STATS")) {
        fprintf(stderr, "zap-gc: evacuation moved %zu objects from %d source blocks\n",
                zap_evac_moved, src);
    }
    /* Enforce the optional hard cap (ZAP_GC_HEAP_LIMIT) against the LIVE set, not
     * mapped memory: freed blocks are retained for reuse, so mapped bytes never
     * shrink, but a correct collector keeps the live set bounded. Exceeding the
     * cap after a full collection means genuinely too much live data (or a
     * collector bug that retains garbage). Used by the GC reclamation tests. */
    if (zap_heap_limit && zap_live_estimate > zap_heap_limit) {
        fprintf(stderr, "zap: live heap %zu exceeds ZAP_GC_HEAP_LIMIT %zu\n",
                zap_live_estimate, zap_heap_limit);
        abort();
    }
    /* Grow the threshold toward ~2x the live set to amortize collection cost,
     * but never beyond the hard cap so the cap still forces timely collections. */
    size_t want = zap_live_estimate * 2;
    if (want < 4u * 1024u * 1024u) {
        want = 4u * 1024u * 1024u;
    }
    if (zap_heap_limit && want > zap_heap_limit) {
        want = zap_heap_limit;
    }
    zap_gc_threshold = want;
}

/* ---- finalizer self-test (ticket #92) ------------------------------------- *
 * Internal hook behind the `__gc_finalizer_selftest()` builtin. Wiring the real
 * CPython `Py_DECREF` path into an automated assertion is impractical (the v1
 * Python interop marshals only ints/floats/strs through module functions and
 * caches modules, so a decref has no deterministic, observable effect), so this
 * exercises the identical registration + post-mark queue + one-shot run machinery
 * with a stand-in finalizer whose side effect is a counter. See the PR body.
 *
 * It allocates N objects, registers an incrementing finalizer on each, and keeps
 * no reference to them (C locals are not GC roots under the precise collector),
 * so a forced collection finds them all unreachable and runs every finalizer. A
 * second collection proves the one-shot contract: finalizers do not run twice.
 * Returns the number of finalizers that ran (N on success). */
static volatile int64_t zap_fin_selftest_ran = 0;

static void zap_fin_selftest_cb(void *obj, void *client_data) {
    (void)obj;
    (void)client_data;
    pthread_mutex_lock(&zap_fin_mu);
    zap_fin_selftest_ran++;
    pthread_mutex_unlock(&zap_fin_mu);
}

int64_t zap_gc_finalizer_selftest(void) {
    enum { N = 64 };
    zap_mutator_register();
    pthread_mutex_lock(&zap_fin_mu);
    zap_fin_selftest_ran = 0;
    pthread_mutex_unlock(&zap_fin_mu);
    for (int i = 0; i < N; i++) {
        void *o = zap_gc_alloc_atomic(16);
        zap_gc_register_finalizer(o, zap_fin_selftest_cb, NULL);
        /* `o` dies with this iteration; it is not a GC root. */
    }
    zap_gc_collect(); /* all N unreachable: finalizers queued + run             */
    zap_gc_collect(); /* reclaim them; one-shot means no finalizer runs again   */
    pthread_mutex_lock(&zap_fin_mu);
    int64_t ran = zap_fin_selftest_ran;
    pthread_mutex_unlock(&zap_fin_mu);
    return ran;
}
