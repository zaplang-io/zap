/* Runtime data-race detector — happens-before / vector-clock engine
 * (#678, slice 2 of epic #665; D-0114; epic #661).
 *
 * Replaces slice 1's no-op `__zap_race_read`/`__zap_race_write` stubs with a
 * real Zap-native happens-before detector (Lamport/Fidge–Mattern vector clocks,
 * the Go/TSan model). Under `--race`:
 *
 *   - every coroutine `G` carries a vector clock, indexed by a dense goroutine
 *     id (`G.race_gid`) the engine assigns at spawn;
 *   - each synchronization object (a `Mutex`) carries a clock used for
 *     release/acquire;
 *   - each instrumented module-global slot keeps shadow state: a write clock
 *     (last write epoch per coroutine) and a read clock (last read epoch per
 *     coroutine), plus the most recent writer/reader's source location for the
 *     report.
 *
 * An access RACES iff it is NOT ordered-after every conflicting prior access
 * (write vs write, or read vs write) under happens-before — i.e. some other
 * coroutine's recorded conflicting epoch exceeds what the accessing coroutine's
 * clock has observed for it. On the first such access the engine prints a
 * diagnostic naming the racing global and both accesses' source spans, then
 * exits the process non-zero (66, Go's race-exit code). A clean run adds no
 * output and preserves the program's own exit code.
 *
 * Happens-before edges wired in this slice (the minimal set for the #665 v1
 * "Done when"):
 *   - spawn: parent's clock -> child at creation (`zap_race_on_spawn`, from
 *     `zap_spawn`/`zap_spawn_detached`);
 *   - Mutex: `unlock` = release (publish the holder's clock into the lock),
 *     `lock` = acquire (join the lock's clock into the locker). Hooked in
 *     `zap_mutex_lock`/`zap_mutex_unlock`.
 *   - Atomic: clean BY EXEMPTION — a `@shareable`/builtin-shareable cell is not
 *     instrumented at all (slice 1), so an `Atomic`-guarded counter needs no
 *     atomic-specific edge here. (Channel / `WaitGroup` / `Once` edges, and
 *     tracking the main/native thread's own accesses, are slice 3 / #679.)
 *
 * ABI — the arguments the slice-1 lowering fixed (`emit_race_access`):
 *   __zap_race_read / __zap_race_write(void *slot, i64 name_id, i64 line,
 *       i64 col): `slot` is the global's data-segment address (the conflict key
 *       is the slot itself); `name_id` is the global's slot index (resolved to a
 *       name via `__zap_race_register_global`); `line`/`col` are the access
 *       site's source byte span (start/end — the names are the slice-1 ABI's,
 *       not true line/column: v1 reports the byte offsets as the location).
 *   __zap_race_enable(void): arm the engine (emitted once at the top of a
 *       `--race` `main`, before any spawn). Dormant until then, so a default
 *       build's `zap_spawn`/`zap_mutex_*` edges are inert and behaviour is
 *       unchanged.
 *   __zap_race_register_global(i64 idx, ZapStr *name): record a global's name
 *       under its slot index for report resolution (every global, emitted at the
 *       top of a `--race` `main`).
 *
 * SERIALIZATION. A single global `race_mu` guards ALL engine state — every
 * vector clock, every slot's shadow state, the gid/name tables. This is a
 * test-time diagnostic: it favours obvious correctness over speed, and sidesteps
 * the hardest native-detector bug (racing on the detector's own metadata under
 * real `MAXPROCS>1`). The happens-before ORDER between a mutex release and the
 * matching acquire comes from the real `Mutex` (and the scheduler's direct
 * handoff), not from `race_mu`; `race_mu` only makes each engine update atomic.
 *
 * These are REAL exported entry points (codegen emits calls to the read/write/
 * enable/register symbols), collector-independent — one definition links into
 * both the Boehm and the Immix runtime. Engine tables use plain malloc/realloc
 * (they hold no GC-managed pointers: registered names are copied into C
 * strings), so the collector never traces them. */

/* ---- vector clock -------------------------------------------------------- */

/* A logical-clock vector. Entries at index >= `len` are implicitly 0, so a
 * clock grows lazily as new goroutine ids appear — no all-clocks resize when a
 * gid is added. */
typedef struct {
    int64_t *v;
    int64_t len;
    int64_t cap;
} RaceVC;

static void race_vc_reserve(RaceVC *c, int64_t n) {
    if (n <= c->cap) {
        return;
    }
    int64_t ncap = c->cap ? c->cap : 4;
    while (ncap < n) {
        ncap *= 2;
    }
    int64_t *nv = (int64_t *)realloc(c->v, (size_t)ncap * sizeof(int64_t));
    if (!nv) {
        fprintf(stderr, "zap --race: out of memory growing a vector clock\n");
        fflush(stderr);
        _exit(70);
    }
    for (int64_t i = c->cap; i < ncap; i++) {
        nv[i] = 0;
    }
    c->v = nv;
    c->cap = ncap;
}

static int64_t race_vc_get(const RaceVC *c, int64_t i) {
    return (i >= 0 && i < c->len) ? c->v[i] : 0;
}

static void race_vc_set(RaceVC *c, int64_t i, int64_t val) {
    race_vc_reserve(c, i + 1);
    for (int64_t k = c->len; k <= i; k++) {
        c->v[k] = 0;
    }
    if (i + 1 > c->len) {
        c->len = i + 1;
    }
    c->v[i] = val;
}

/* dst := element-wise max(dst, src) — the join used by acquire/inherit. */
static void race_vc_join(RaceVC *dst, const RaceVC *src) {
    for (int64_t i = 0; i < src->len; i++) {
        if (src->v[i] > race_vc_get(dst, i)) {
            race_vc_set(dst, i, src->v[i]);
        }
    }
}

/* dst := copy of src (used when a child inherits its parent's clock). */
static void race_vc_copy(RaceVC *dst, const RaceVC *src) {
    dst->len = 0;
    race_vc_reserve(dst, src->len);
    for (int64_t i = 0; i < src->len; i++) {
        dst->v[i] = src->v[i];
    }
    dst->len = src->len;
}

/* ---- engine state (all under race_mu) ------------------------------------ */

static pthread_mutex_t race_mu = PTHREAD_MUTEX_INITIALIZER;
static int race_enabled = 0; /* set once by __zap_race_enable, before any spawn */

/* Per-goroutine clocks, indexed by the dense `G.race_gid`. */
static RaceVC *race_gvc = NULL;
static int64_t race_ngids = 0;  /* number of ids handed out (== used length) */
static int64_t race_gvc_cap = 0;

/* name_id (global slot index) -> owned C-string copy of the global's name. */
static char **race_names = NULL;
static int64_t race_names_len = 0;
static int64_t race_names_cap = 0;

/* Per-instrumented-slot shadow state, keyed by the slot address. A handful of
 * module globals today, so a flat array with linear lookup is plenty. */
typedef struct {
    const void *slot;
    int64_t name_id;
    RaceVC w; /* w[g] = epoch of coroutine g's last write to this slot  */
    RaceVC r; /* r[g] = epoch of coroutine g's last read of this slot   */
    int64_t last_wgid, last_wline, last_wcol; /* most recent writer (report) */
    int64_t last_rgid, last_rline, last_rcol; /* most recent reader (report) */
    /* Spawn-target name of the most recent writer/reader coroutine, for the
     * report. `G.name` is a static C-string (a codegen global or runtime
     * literal) valid for the process lifetime, so it is safe to retain without
     * copying and never GC-traced. NULL for a coroutine with no recorded name. */
    const char *last_wname;
    const char *last_rname;
} RaceSlot;

static RaceSlot *race_slots = NULL;
static int64_t race_slots_len = 0;
static int64_t race_slots_cap = 0;

/* Per-sync-object clock (a `Mutex`), keyed by the object pointer. */
typedef struct {
    const void *key;
    RaceVC c;
} RaceSync;

static RaceSync *race_syncs = NULL;
static int64_t race_syncs_len = 0;
static int64_t race_syncs_cap = 0;

/* Allocate a fresh dense goroutine id with an empty clock. Caller holds race_mu.
 * May realloc `race_gvc`, so callers must re-take any `&race_gvc[...]` pointer
 * AFTER this returns. */
static int64_t race_new_gid(void) {
    int64_t id = race_ngids++;
    if (id >= race_gvc_cap) {
        int64_t ncap = race_gvc_cap ? race_gvc_cap * 2 : 8;
        RaceVC *ng = (RaceVC *)realloc(race_gvc, (size_t)ncap * sizeof(RaceVC));
        if (!ng) {
            fprintf(stderr, "zap --race: out of memory growing the clock table\n");
            fflush(stderr);
            _exit(70);
        }
        race_gvc = ng;
        race_gvc_cap = ncap;
    }
    race_gvc[id].v = NULL;
    race_gvc[id].len = 0;
    race_gvc[id].cap = 0;
    return id;
}

/* The clock for a specific coroutine `g`, or NULL for the main/native thread
 * (no `G`), which stays untracked in this slice (so a clean fixture whose `main`
 * reads a global after a `WaitGroup` join stays clean without a main-thread
 * edge; a genuine main-vs-coroutine race is the documented v1/slice-3
 * limitation). Factored out of `race_cur_vc` so the slice-3 happens-before edges
 * (channel / `WaitGroup` / `Once`) can also reach a *parked* counterpart
 * coroutine — a channel peer blocked in `copark`, a `wait()`er — that is not the
 * one currently running. Assigns a dense gid (with a fresh own-component) the
 * first time the engine sees `g`. Caller holds race_mu; MAY realloc `race_gvc`,
 * so callers must re-take any `&race_gvc[...]` pointer AFTER this returns. */
static RaceVC *race_vc_of_g(G *g) {
    if (!g) {
        return NULL; /* main/native thread — untracked (slice 3 keeps main out) */
    }
    if (g->race_gid < 0) {
        /* First time the engine sees this coroutine. `zap_race_on_spawn` assigns
         * the id (and the parent->child edge) when the spawn path is hooked; this
         * lazy init is the fallback for any spawn path not hooked, so a coroutine
         * is tracked the moment it touches a shared global — or is reached by a
         * slice-3 handoff edge — regardless of how it was created. A lazily
         * initialized clock has no inherited edges (fresh own component), which is
         * conservative: it can only ever ADD ordering, never hide a race. */
        int64_t id = race_new_gid();
        g->race_gid = id;
        race_vc_set(&race_gvc[id], id, 1);
    }
    if (g->race_gid >= race_ngids) {
        return NULL; /* defensive: should never happen */
    }
    return &race_gvc[g->race_gid];
}

/* The currently-running coroutine's clock, or NULL when the running context is
 * the main/native thread. A NULL result means "untracked". Caller holds
 * race_mu. */
static RaceVC *race_cur_vc(G **out_g) {
    M *m = cur_m;
    G *g = m ? m->current : NULL;
    if (out_g) {
        *out_g = g;
    }
    return race_vc_of_g(g);
}

/* Get-or-create the shadow state for `slot`. Caller holds race_mu. May realloc
 * `race_slots`; the returned pointer is valid until the next slot is added. */
static RaceSlot *race_slot(const void *slot, int64_t name_id) {
    for (int64_t i = 0; i < race_slots_len; i++) {
        if (race_slots[i].slot == slot) {
            return &race_slots[i];
        }
    }
    if (race_slots_len == race_slots_cap) {
        int64_t ncap = race_slots_cap ? race_slots_cap * 2 : 8;
        RaceSlot *ns = (RaceSlot *)realloc(race_slots, (size_t)ncap * sizeof(RaceSlot));
        if (!ns) {
            fprintf(stderr, "zap --race: out of memory growing slot shadow state\n");
            fflush(stderr);
            _exit(70);
        }
        race_slots = ns;
        race_slots_cap = ncap;
    }
    RaceSlot *s = &race_slots[race_slots_len++];
    memset(s, 0, sizeof(*s));
    s->slot = slot;
    s->name_id = name_id;
    s->last_wgid = s->last_rgid = -1;
    return s;
}

/* Get-or-create the clock for a sync object. Caller holds race_mu. */
static RaceVC *race_sync_vc(const void *key) {
    for (int64_t i = 0; i < race_syncs_len; i++) {
        if (race_syncs[i].key == key) {
            return &race_syncs[i].c;
        }
    }
    if (race_syncs_len == race_syncs_cap) {
        int64_t ncap = race_syncs_cap ? race_syncs_cap * 2 : 8;
        RaceSync *ns = (RaceSync *)realloc(race_syncs, (size_t)ncap * sizeof(RaceSync));
        if (!ns) {
            fprintf(stderr, "zap --race: out of memory growing sync clocks\n");
            fflush(stderr);
            _exit(70);
        }
        race_syncs = ns;
        race_syncs_cap = ncap;
    }
    RaceSync *e = &race_syncs[race_syncs_len++];
    e->key = key;
    e->c.v = NULL;
    e->c.len = 0;
    e->c.cap = 0;
    return &e->c;
}

static const char *race_name_of(int64_t name_id) {
    if (name_id >= 0 && name_id < race_names_len && race_names[name_id]) {
        return race_names[name_id];
    }
    return "<global>";
}

/* Print the race report and terminate the process non-zero. Caller holds
 * race_mu (we never return, so it is not released). `_exit` avoids running
 * atexit / GC teardown from inside a worker coroutine holding the detector lock.
 * The report prints the global's NAME and the accesses' source byte spans only —
 * never a heap address, pointer, or timing value — so a golden can assert it
 * without flaking. */
static void race_report_and_exit(const char *kind, int64_t name_id, int64_t gid, int64_t line,
                                 int64_t col, const char *cur_name, const char *prior_kind,
                                 int64_t pgid, int64_t pline, int64_t pcol, const char *prior_name) {
    const char *gname = race_name_of(name_id);
    fflush(stdout);
    fprintf(stderr, "==================\n");
    fprintf(stderr, "zap: data race detected on module global `%s` (--race, #665)\n", gname);
    /* Each racing access names the coroutine (dense id + spawn-target function,
     * when known) and the access-site source byte span. The coroutine function
     * is a deterministic, address-free identifier, so it enriches the report
     * without flaking a golden. Full per-access multi-frame stack traces need the
     * green-stack unwinder that I#475 tracks as a follow-up and are deferred with
     * it (see tests/race/README.md). */
    if (cur_name) {
        fprintf(stderr, "  %s by coroutine %lld (`%s`) at source bytes %lld..%lld\n", kind,
                (long long)gid, cur_name, (long long)line, (long long)col);
    } else {
        fprintf(stderr, "  %s by coroutine %lld at source bytes %lld..%lld\n", kind, (long long)gid,
                (long long)line, (long long)col);
    }
    if (pgid >= 0) {
        if (prior_name) {
            fprintf(stderr, "  previous %s by coroutine %lld (`%s`) at source bytes %lld..%lld\n",
                    prior_kind, (long long)pgid, prior_name, (long long)pline, (long long)pcol);
        } else {
            fprintf(stderr, "  previous %s by coroutine %lld at source bytes %lld..%lld\n",
                    prior_kind, (long long)pgid, (long long)pline, (long long)pcol);
        }
    }
    fprintf(stderr, "  no happens-before edge orders these accesses\n");
    fprintf(stderr, "==================\n");
    fflush(stderr);
    _exit(66);
}

/* ---- generated-code entry points ----------------------------------------- */

void __zap_race_enable(void) {
    /* Arm the engine. Runs on the main thread at the top of a `--race` `main`,
     * before any `spawn`, so a plain store is safely published to the worker
     * threads the first spawn creates. Idempotent, and belt-and-suspenders: the
     * access hooks and `__zap_race_register_global` also self-arm, so the engine
     * is live even if this call is reordered or elided — `race_enabled` only
     * gates the runtime-side (always-linked) spawn / Mutex hooks, keeping a
     * non-`--race` build (which emits none of these calls) inert. */
    race_enabled = 1;
}

void __zap_race_register_global(int64_t name_id, void *name_) {
    const ZapStr *s = (const ZapStr *)name_;
    /* Emitted per-global in a `--race` `main` prologue, before any spawn or
     * Mutex op — so arming here guarantees the runtime-side spawn / Mutex edges
     * are live in time, independent of `__zap_race_enable`. */
    race_enabled = 1;
    zap_np_lock(&race_mu);
    if (name_id >= race_names_cap) {
        int64_t ncap = race_names_cap ? race_names_cap * 2 : 8;
        while (ncap <= name_id) {
            ncap *= 2;
        }
        char **nn = (char **)realloc(race_names, (size_t)ncap * sizeof(char *));
        if (!nn) {
            zap_np_unlock(&race_mu);
            return; /* a missing name just degrades the report to "<global>" */
        }
        for (int64_t i = race_names_cap; i < ncap; i++) {
            nn[i] = NULL;
        }
        race_names = nn;
        race_names_cap = ncap;
    }
    if (name_id + 1 > race_names_len) {
        race_names_len = name_id + 1;
    }
    /* Copy the bytes: the `ZapStr` literal is fine to point at for the process
     * lifetime, but copying keeps the engine independent of string
     * representation and gives us a NUL-terminated C string to print. */
    if (s && !race_names[name_id]) {
        int64_t n = s->len < 0 ? 0 : s->len;
        char *buf = (char *)malloc((size_t)n + 1);
        if (buf) {
            if (n > 0) {
                memcpy(buf, s->data, (size_t)n);
            }
            buf[n] = '\0';
            race_names[name_id] = buf;
        }
    }
    zap_np_unlock(&race_mu);
}

void __zap_race_read(void *slot, int64_t name_id, int64_t line, int64_t col) {
    /* An access hook is emitted ONLY under `--race`, so its mere presence means
     * the detector is live: self-arm (also arms the runtime-side spawn / Mutex
     * edges) rather than depend on `__zap_race_enable` having run first. */
    race_enabled = 1;
    zap_np_lock(&race_mu);
    G *g = NULL;
    RaceVC *cvc = race_cur_vc(&g);
    if (!cvc) {
        zap_np_unlock(&race_mu); /* untracked context (main/native) */
        return;
    }
    int64_t gid = g->race_gid;
    RaceSlot *s = race_slot(slot, name_id);
    /* write->read: a write by another coroutine that this one has not observed. */
    for (int64_t u = 0; u < s->w.len; u++) {
        if (u != gid && s->w.v[u] > race_vc_get(cvc, u)) {
            race_report_and_exit("read", name_id, gid, line, col, g->name, "write", s->last_wgid,
                                  s->last_wline, s->last_wcol, s->last_wname);
        }
    }
    race_vc_set(&s->r, gid, race_vc_get(cvc, gid));
    s->last_rgid = gid;
    s->last_rline = line;
    s->last_rcol = col;
    s->last_rname = g->name;
    zap_np_unlock(&race_mu);
}

void __zap_race_write(void *slot, int64_t name_id, int64_t line, int64_t col) {
    race_enabled = 1; /* self-arm (see __zap_race_read) */
    zap_np_lock(&race_mu);
    G *g = NULL;
    RaceVC *cvc = race_cur_vc(&g);
    if (!cvc) {
        zap_np_unlock(&race_mu); /* untracked context (main/native) */
        return;
    }
    int64_t gid = g->race_gid;
    RaceSlot *s = race_slot(slot, name_id);
    /* write->write: an unobserved write by another coroutine. */
    for (int64_t u = 0; u < s->w.len; u++) {
        if (u != gid && s->w.v[u] > race_vc_get(cvc, u)) {
            race_report_and_exit("write", name_id, gid, line, col, g->name, "write", s->last_wgid,
                                  s->last_wline, s->last_wcol, s->last_wname);
        }
    }
    /* read->write: an unobserved read by another coroutine (a write that is not
     * ordered after a concurrent reader still races). */
    for (int64_t u = 0; u < s->r.len; u++) {
        if (u != gid && s->r.v[u] > race_vc_get(cvc, u)) {
            race_report_and_exit("write", name_id, gid, line, col, g->name, "read", s->last_rgid,
                                  s->last_rline, s->last_rcol, s->last_rname);
        }
    }
    race_vc_set(&s->w, gid, race_vc_get(cvc, gid));
    s->last_wgid = gid;
    s->last_wline = line;
    s->last_wcol = col;
    s->last_wname = g->name;
    zap_np_unlock(&race_mu);
}

/* ---- happens-before edges (called from the scheduler / sync primitives) --- */

void zap_race_on_spawn(G *child) {
    if (!race_enabled) {
        return;
    }
    zap_np_lock(&race_mu);
    M *m = cur_m;
    G *pg = m ? m->current : NULL;
    int64_t pgid = (pg && pg->race_gid >= 0) ? pg->race_gid : -1;
    int64_t cid = race_new_gid(); /* may realloc race_gvc -> index AFTER */
    child->race_gid = cid;
    RaceVC *cvc = &race_gvc[cid];
    if (pgid >= 0) {
        RaceVC *pvc = &race_gvc[pgid];
        race_vc_copy(cvc, pvc); /* child inherits everything the parent knows */
        /* advance the parent's own clock: its post-spawn actions are ordered
         * after the fork point (and distinct from what the child inherited). */
        race_vc_set(pvc, pgid, race_vc_get(pvc, pgid) + 1);
    }
    race_vc_set(cvc, cid, 1); /* the child's own component starts at 1 */
    zap_np_unlock(&race_mu);
}

void zap_race_mutex_acquire(void *mu) {
    if (!race_enabled) {
        return;
    }
    zap_np_lock(&race_mu);
    G *g = NULL;
    RaceVC *cvc = race_cur_vc(&g);
    if (cvc) {
        RaceVC *lc = race_sync_vc(mu);
        race_vc_join(cvc, lc); /* acquire: observe everything the lock published */
    }
    zap_np_unlock(&race_mu);
}

void zap_race_mutex_release(void *mu) {
    if (!race_enabled) {
        return;
    }
    zap_np_lock(&race_mu);
    G *g = NULL;
    RaceVC *cvc = race_cur_vc(&g);
    if (cvc) {
        RaceVC *lc = race_sync_vc(mu);
        race_vc_join(lc, cvc); /* release: publish the holder's clock into the lock */
        /* advance the holder's own clock so its next critical section is a new
         * epoch (standard release-increment; keeps critical sections distinct). */
        race_vc_set(cvc, g->race_gid, race_vc_get(cvc, g->race_gid) + 1);
    }
    zap_np_unlock(&race_mu);
}

/* ---- slice 3 (#679): channel / WaitGroup / Once happens-before edges ------
 *
 * These widen the engine from the slice-2 minimum (spawn + `Mutex`) to the rest
 * of the light model's synchronization. Like the `Mutex` edges they are
 * RUNTIME-ONLY: the scheduler and the `sync.*` primitives call them directly
 * (codegen emits nothing new), so a build without `--race` is byte-identical and
 * these hooks are inert until an access hook / `__zap_race_register_global`
 * arms the engine. All engine state stays under the single `race_mu`; the real
 * happens-before ORDER comes from the primitive's own lock (the channel mutex,
 * the `WaitGroup`/`Once` mutex) publishing before it wakes a waiter, exactly as
 * the `Mutex` release/acquire pair does. */

/* Generic release/acquire against a per-sync-object clock keyed by `key`, using
 * the CURRENT coroutine's clock. `release_into` publishes (the object's clock
 * learns everything the caller knows); `acquire_from` observes (the caller
 * learns everything the object published). No release-increment: unlike a
 * `Mutex`'s repeated critical sections, a `WaitGroup`/`Once` completion is a
 * one-shot edge, so a plain join is exactly the guarantee. Caller holds
 * race_mu. */
static void race_release_into(const void *key) {
    G *g = NULL;
    RaceVC *cvc = race_cur_vc(&g);
    if (cvc) {
        RaceVC *sc = race_sync_vc(key);
        race_vc_join(sc, cvc);
    }
}
static void race_acquire_from(const void *key) {
    G *g = NULL;
    RaceVC *cvc = race_cur_vc(&g);
    if (cvc) {
        RaceVC *sc = race_sync_vc(key);
        race_vc_join(cvc, sc);
    }
}

/* Channel rendezvous (unbuffered, or the direct sender<->waiting-receiver
 * hand-off a buffered channel also takes): the value carries a happens-before
 * edge from the sending coroutine to the receiving one. `from_g`/`to_g` are the
 * two `G`s identified at the hand-off (one is running, the other parked in
 * `copark`); the receiver's clock learns everything the sender knew. A
 * main/native-thread peer (`G*` NULL) is untracked and simply skipped. Both gids
 * are assigned BEFORE either `&race_gvc[...]` is taken, because `race_vc_of_g`
 * may realloc `race_gvc`. */
void zap_race_chan_xfer(void *from_g, void *to_g) {
    if (!race_enabled) {
        return;
    }
    zap_np_lock(&race_mu);
    G *fg = (G *)from_g;
    G *tg = (G *)to_g;
    (void)race_vc_of_g(fg); /* assign gids first (may realloc race_gvc) */
    (void)race_vc_of_g(tg);
    if (fg && tg && fg->race_gid >= 0 && tg->race_gid >= 0 && fg->race_gid < race_ngids &&
        tg->race_gid < race_ngids && fg->race_gid != tg->race_gid) {
        race_vc_join(&race_gvc[tg->race_gid], &race_gvc[fg->race_gid]);
    }
    zap_np_unlock(&race_mu);
}

/* Buffered hand-off. A value sits in a ring-buffer slot between its send and the
 * matching receive, so the edge is split in two, keyed by the physical slot
 * address (one live value per slot at a time; FIFO order means the k-th value
 * sent is the k-th received):
 *   - `buf_store` publishes the producer's clock into the slot (OVERWRITE, not
 *     join, so the slot carries exactly this value's producer — a later reuse of
 *     the same physical slot by a different value does NOT fabricate an edge from
 *     the previous receiver, which would be unsound). `from_g` is the producer:
 *     the running sender on a plain buffered send, or the parked sender whose
 *     value a receiver pulls forward on a buffer refill.
 *   - `buf_load` joins the slot's clock into the consuming coroutine.
 * This establishes send-of-value-k -> receive-of-value-k. The weaker
 * back-pressure edge (receive-k -> send-of-k+capacity) is deliberately NOT
 * modeled: omitting a real edge can only make the detector MORE eager to report
 * (never hide a race), which is the safe direction for a test-time tool. */
void zap_race_chan_buf_store(void *slot, void *from_g) {
    if (!race_enabled) {
        return;
    }
    zap_np_lock(&race_mu);
    RaceVC *fv = race_vc_of_g((G *)from_g);
    if (fv) {
        RaceVC *cell = race_sync_vc(slot);
        race_vc_copy(cell, fv);
    }
    zap_np_unlock(&race_mu);
}
void zap_race_chan_buf_load(void *slot, void *to_g) {
    if (!race_enabled) {
        return;
    }
    zap_np_lock(&race_mu);
    RaceVC *tv = race_vc_of_g((G *)to_g);
    if (tv) {
        RaceVC *cell = race_sync_vc(slot);
        race_vc_join(tv, cell);
    }
    zap_np_unlock(&race_mu);
}

/* WaitGroup: every `done()` (the decrement, `add` with n < 0) publishes the
 * caller's clock into the group's clock; every `wait()` that returns observes
 * it. So all the `done()`s happen-before a `wait()` returns — the D-0032 /
 * design-0005 WaitGroup guarantee. `add(+n)` does NOT publish (an incrementing
 * `add` is not part of the join edge), keeping the edge precise. */
void zap_race_wg_release(void *wg) {
    if (!race_enabled) {
        return;
    }
    zap_np_lock(&race_mu);
    race_release_into(wg);
    zap_np_unlock(&race_mu);
}
void zap_race_wg_acquire(void *wg) {
    if (!race_enabled) {
        return;
    }
    zap_np_lock(&race_mu);
    race_acquire_from(wg);
    zap_np_unlock(&race_mu);
}

/* Once: the winner's `once_end` publishes its clock (its initializer ran before
 * it); every loser's `once_begin` that returns 0 — whether it parked until
 * completion or arrived after — observes it. So the initializer happens-before
 * every `once_begin` return past it. The winning `once_begin` (returns 1) needs
 * no edge: it runs the initializer itself. */
void zap_race_once_release(void *once) {
    if (!race_enabled) {
        return;
    }
    zap_np_lock(&race_mu);
    race_release_into(once);
    zap_np_unlock(&race_mu);
}
void zap_race_once_acquire(void *once) {
    if (!race_enabled) {
        return;
    }
    zap_np_lock(&race_mu);
    race_acquire_from(once);
    zap_np_unlock(&race_mu);
}
