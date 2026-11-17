/* ===========================================================================
 * sync — coroutine synchronization primitives (ticket #103, D-0032)
 * ---------------------------------------------------------------------------
 * Mutex, WaitGroup, and Once, built on the scheduler's ONE park/wake primitive
 * (`copark`/`coready`) and `Waiter` queue (defined with the channel runtime
 * above) — exactly as channels and the netpoller do. A coroutine that blocks on
 * a contended lock, a `wait()`, or an in-flight `Once` therefore *parks its G
 * and frees its M* rather than tying up an OS thread (design/0005 "a blocked
 * coroutine parks its G"); a non-coroutine (main) thread falls back to a condvar
 * inside `copark` without consuming a worker M.
 *
 * Each handle is a GC object with an embedded `pthread_mutex_t` guarding its
 * small critical section plus a FIFO `Waiter` queue. The handle carries no
 * traced GC pointers of its own: the only pointers it holds are the wait-queue
 * head/tail, and every queued `Waiter` is a stack local of a blocked coroutine,
 * rooted via that coroutine's shadow chain (same arrangement as a channel's
 * send/recv queues, which `ZAP_TD_CHAN` likewise does not trace). So the
 * descriptors are `ZAP_TD_ATOMIC` — nothing inside to scan. The embedded mutex
 * is safe to address across a park because v1 Immix is non-moving (see the GC
 * header) — identical to a channel embedding its own `pthread_mutex_t`.
 *
 * Deferred (noted in the PR, D-0032 completeness): `RWMutex`, lock-free
 * `Atomic[T]` (needs the generic `sync.Atomic[int64]` surface + memory-model
 * atomic/fence lowering), and the exception-safe `with mu.lock():` /
 * `with wg.tracked():` guards (blocked on general context managers, D-0027;
 * `with` currently supports only `expect_raises`). Mutexes are non-reentrant
 * (re-locking deadlocks — a bug, per D-0032).
 * ========================================================================== */

/* ---- Mutex: non-reentrant, FIFO, parks the waiter --------------------------
 * `locked` is handed off directly on unlock: `unlock` dequeues one waiter and
 * wakes it *without clearing* `locked`, so the woken waiter owns the lock on
 * return from `copark` with no re-check and no acquire race. */
typedef struct {
    pthread_mutex_t mu;           /* guards `locked` + the wait queue           */
    int locked;                   /* 1 iff the lock is held                     */
    Waiter *wq_head, *wq_tail;    /* coroutines/threads waiting to acquire      */
} ZapMutex;

static const ZapTypeDesc zap_desc_mutex = {(int64_t)sizeof(ZapMutex), ZAP_TD_ATOMIC, 0, NULL};

void *zap_mutex_new(void) {
    ZapMutex *m = (ZapMutex *)zap_obj(&zap_desc_mutex);
    pthread_mutex_init(&m->mu, NULL);
    m->locked = 0;
    m->wq_head = m->wq_tail = NULL;
    return m;
}

void zap_mutex_lock(void *mp) {
    ZapMutex *m = (ZapMutex *)mp;
    zap_np_lock(&m->mu);
    if (!m->locked) {
        m->locked = 1;
        zap_np_unlock(&m->mu);
        /* --race: acquire edge — join the lock's published clock into this
         * coroutine's vector clock (#678). The previous holder's `unlock`
         * published under the real mutex, so the edge is ordered. */
        zap_race_mutex_acquire(mp);
        return;
    }
    /* Contended: enqueue and park. On wake the lock has been handed to us
     * directly (see `zap_mutex_unlock`); `copark` releases `m->mu`. */
    Waiter w;
    w.g = NULL;
    q_push(&m->wq_head, &m->wq_tail, &w);
    copark(&m->mu, &w);
    /* Woken with ownership (direct handoff): acquire the lock's clock now. */
    zap_race_mutex_acquire(mp);
}

/* Try to acquire without blocking: 1 on success, 0 if already held. */
int64_t zap_mutex_trylock(void *mp) {
    ZapMutex *m = (ZapMutex *)mp;
    zap_np_lock(&m->mu);
    int64_t got = 0;
    if (!m->locked) {
        m->locked = 1;
        got = 1;
    }
    zap_np_unlock(&m->mu);
    return got;
}

void zap_mutex_unlock(void *mp) {
    ZapMutex *m = (ZapMutex *)mp;
    /* --race: release edge — publish this coroutine's vector clock into the
     * lock's clock BEFORE the direct handoff below, so the next locker's acquire
     * sees it (#678). Runs before `coready`, so the publish happens-before the
     * woken waiter's acquire. */
    zap_race_mutex_release(mp);
    zap_np_lock(&m->mu);
    Waiter *nx = q_pop(&m->wq_head, &m->wq_tail);
    if (nx) {
        /* Direct handoff: ownership passes to `nx`; `locked` stays 1. */
        coready(nx);
    } else {
        m->locked = 0;
    }
    zap_np_unlock(&m->mu);
}

/* ---- WaitGroup: the primary join for fire-and-forget `spawn`s (D-0032) -----
 * `wait()` parks until the counter returns to 0. `done()` is `add(-1)`. A
 * counter driven below 0 is a bug and panics (Go semantics). */
typedef struct {
    pthread_mutex_t mu;
    int64_t count;
    Waiter *wq_head, *wq_tail;    /* coroutines/threads blocked in `wait()`     */
} ZapWaitGroup;

static const ZapTypeDesc zap_desc_waitgroup = {(int64_t)sizeof(ZapWaitGroup), ZAP_TD_ATOMIC, 0,
                                               NULL};

void *zap_waitgroup_new(void) {
    ZapWaitGroup *w = (ZapWaitGroup *)zap_obj(&zap_desc_waitgroup);
    pthread_mutex_init(&w->mu, NULL);
    w->count = 0;
    w->wq_head = w->wq_tail = NULL;
    return w;
}

void zap_waitgroup_add(void *wp, int64_t n) {
    ZapWaitGroup *w = (ZapWaitGroup *)wp;
    zap_np_lock(&w->mu);
    w->count += n;
    if (w->count < 0) {
        zap_np_unlock(&w->mu);
        zap_panic(zap_str_new("negative WaitGroup counter", 26));
        return;
    }
    /* --race: a `done()` (decrement) is a release — publish the caller's clock
     * into the group so every `wait()` that returns observes it. Done under
     * `w->mu` and before any `coready` below, so the publish happens-before a
     * woken waiter's acquire (#679). An incrementing `add` carries no edge. */
    if (n < 0) {
        zap_race_wg_release(wp);
    }
    if (w->count == 0) {
        /* Release every waiter; each returns from its `copark`. */
        Waiter *x;
        while ((x = q_pop(&w->wq_head, &w->wq_tail)) != NULL) {
            coready(x);
        }
    }
    zap_np_unlock(&w->mu);
}

void zap_waitgroup_done(void *wp) { zap_waitgroup_add(wp, -1); }

void zap_waitgroup_wait(void *wp) {
    ZapWaitGroup *w = (ZapWaitGroup *)wp;
    zap_np_lock(&w->mu);
    if (w->count == 0) {
        zap_np_unlock(&w->mu);
        /* --race: acquire the group's clock — the fast path still joins every
         * `done()` that already brought the counter to 0 (#679). */
        zap_race_wg_acquire(wp);
        return;
    }
    Waiter x;
    x.g = NULL;
    q_push(&w->wq_head, &w->wq_tail, &x);
    copark(&w->mu, &x); /* releases `w->mu`; woken when count hits 0 */
    /* --race: woken (counter reached 0) — acquire the group's clock, ordering
     * this waiter after every `done()` (#679). */
    zap_race_wg_acquire(wp);
}

/* ---- Once: run an initializer exactly once across all coroutines -----------
 * No first-class functions yet (so no `once.do(fn)`), so the surface is a
 * begin/end pair:
 *
 *     if sync.once_begin(o):     # true for exactly one caller
 *         init()
 *         sync.once_end(o)       # completion is visible to every waiter
 *
 * `once_begin` returns 1 to the single winning caller and 0 to the rest; a
 * loser that arrives while `init()` is still running parks until `once_end`, so
 * the completion of `init()` happens-before the return of every `once_begin`
 * (D-0032 / design/0005 happens-before rule for `Once.do`). A loser arriving
 * after completion returns 0 immediately. */
typedef struct {
    pthread_mutex_t mu;
    int state;                    /* 0 = fresh, 1 = running, 2 = done           */
    Waiter *wq_head, *wq_tail;    /* losers waiting for the winner to finish     */
} ZapOnce;

static const ZapTypeDesc zap_desc_once = {(int64_t)sizeof(ZapOnce), ZAP_TD_ATOMIC, 0, NULL};

void *zap_once_new(void) {
    ZapOnce *o = (ZapOnce *)zap_obj(&zap_desc_once);
    pthread_mutex_init(&o->mu, NULL);
    o->state = 0;
    o->wq_head = o->wq_tail = NULL;
    return o;
}

int64_t zap_once_begin(void *op) {
    ZapOnce *o = (ZapOnce *)op;
    zap_np_lock(&o->mu);
    if (o->state == 2) {
        zap_np_unlock(&o->mu);
        /* --race: a loser arriving after completion still observes the winner's
         * initializer — acquire the Once clock (#679). */
        zap_race_once_acquire(op);
        return 0; /* already completed */
    }
    if (o->state == 0) {
        o->state = 1;
        zap_np_unlock(&o->mu);
        return 1; /* winner: caller runs init, then calls once_end (no acquire) */
    }
    /* state == 1: a winner is running init; park until it completes. */
    Waiter w;
    w.g = NULL;
    q_push(&o->wq_head, &o->wq_tail, &w);
    copark(&o->mu, &w);
    /* --race: woken by `once_end` — acquire the winner's clock so the
     * initializer happens-before this return (#679). */
    zap_race_once_acquire(op);
    return 0;
}

void zap_once_end(void *op) {
    ZapOnce *o = (ZapOnce *)op;
    /* --race: release the winner's clock (its initializer ran before this) into
     * the Once, published BEFORE waking any loser, so every loser's `once_begin`
     * return is ordered after the initializer (#679). */
    zap_race_once_release(op);
    zap_np_lock(&o->mu);
    o->state = 2;
    Waiter *x;
    while ((x = q_pop(&o->wq_head, &o->wq_tail)) != NULL) {
        coready(x);
    }
    zap_np_unlock(&o->mu);
}
