/* ===========================================================================
 * threading — Python-`threading`-shaped OS-thread compatibility (ticket #175)
 * ---------------------------------------------------------------------------
 * A MIGRATION ESCAPE HATCH, explicitly not recommended: idiomatic Zap
 * concurrency is coroutines + channels. This library lets lift-and-shift Python
 * `threading` code keep OS-thread semantics so a future py->zap transpiler can
 * map it by syntax. A `threading.Thread` is NOT a bare pthread: `Thread.start`
 * does `spawn detached __thread_entry(...)`, and that coroutine immediately pins
 * itself to a dedicated OS thread via `zap_threading_pin` (the #172 LockOSThread
 * substrate, with `ded_joinable = 0`). So the thread participates in precise-GC
 * stop-the-world and safepoints and roots its pointers through the ordinary
 * per-coroutine shadow stack exactly like any coroutine — there is no raw
 * OS-thread stack/register set hidden from the collector, and the #120 moving
 * collector's load-at-use reloads apply to it unchanged. The dedicated M has
 * `p == NULL` and is not in `g_ms[]`, so a Thread runs beyond the MAXPROCS cap
 * and is never multiplexed onto a P.
 *
 * Exit semantics (CPython): a non-daemon Thread is awaited at program exit; a
 * daemon Thread is not. Rather than touch the #172 `g_ded_live` barrier, a
 * non-daemon Thread is registered in `g_live` (the root-scope join counter) in
 * `Thread.start` — synchronously, before the coroutine runs, so there is no
 * start race — and deregistered when its body completes; `zap_rt_join_all`
 * already blocks on `g_live` (GC-safe). A daemon Thread is registered in no
 * barrier, so it is abandoned when `main` returns.
 *
 * Every handle below is a GC object (`zap_obj`) with a `ZAP_TD_ATOMIC`
 * descriptor — it holds no traced GC pointers, only an embedded
 * `pthread_mutex_t` and a FIFO `Waiter` queue whose nodes are stack locals of
 * the blocked coroutines (rooted via their shadow chains), exactly as the `sync`
 * module and channels arrange it. Untimed waits use the shared cooperative
 * `copark`/`coready` primitive (a blocked coroutine parks its G and frees its
 * M). Timed waits (`join`/`wait`/`acquire` with a timeout) have no cooperative
 * scheduler timer to reuse (even `time.sleep` is a blocking `nanosleep`; the
 * #80 deadline path is fd/epoll-bound), so they block the calling OS thread on
 * the waiter's own condvar via `pthread_cond_timedwait` — correct, and
 * semantically apt for an OS-thread library, at the cost of tying up that
 * thread (a worker M, if the caller is an ordinary coroutine) for the wait.
 * ========================================================================== */

#include <errno.h> /* ETIMEDOUT for the timed waits below */

/* Absolute CLOCK_REALTIME deadline `secs` from now (matches the default condvar
 * clock). Negative is clamped to now. */
static void thr_deadline(double secs, struct timespec *ts) {
    clock_gettime(CLOCK_REALTIME, ts);
    if (secs < 0) {
        secs = 0;
    }
    long whole = (long)secs;
    long nsec = ts->tv_nsec + (long)((secs - (double)whole) * 1e9);
    ts->tv_sec += (time_t)whole + nsec / 1000000000L;
    ts->tv_nsec = nsec % 1000000000L;
}

/* Timed park on a handle queue guarded by `mu` (held on entry and on return).
 * Returns 1 if woken by `coready` (the waker set `w->done`), 0 on timeout. The
 * waiter always blocks this OS thread on its own condvar (see the section
 * header); the caller dequeues `w` on timeout. GC-safe while blocked. */
static int copark_timed(pthread_mutex_t *mu, Waiter *w, double secs) {
    w->g = NULL;
    pthread_cond_init(&w->cv, NULL);
    w->done = 0;
    struct timespec ts;
    thr_deadline(secs, &ts);
    zap_enter_blocking();
    int woken = 1;
    while (!w->done) {
        int r = pthread_cond_timedwait(&w->cv, mu, &ts);
        if (r == ETIMEDOUT) {
            woken = w->done ? 1 : 0;
            break;
        }
    }
    zap_leave_blocking();
    return woken;
}

/* Remove `w` from the queue if present (used when a timed waiter gives up). */
static void q_remove(Waiter **head, Waiter **tail, Waiter *w) {
    Waiter *prev = NULL, *cur = *head;
    while (cur) {
        if (cur == w) {
            if (prev) {
                prev->next = cur->next;
            } else {
                *head = cur->next;
            }
            if (*tail == cur) {
                *tail = prev;
            }
            cur->next = NULL;
            return;
        }
        prev = cur;
        cur = cur->next;
    }
}

/* ---- Thread handle: liveness + join ---------------------------------------
 * `started`/`done` drive `is_alive`; `wq` holds coroutines/threads in `join`. */
typedef struct {
    pthread_mutex_t mu;
    int started;               /* Thread.start has been called                 */
    int done;                  /* the thread body has finished                  */
    Waiter *wq_head, *wq_tail; /* joiners parked in join()/join_timeout()       */
} ZapThread;

static const ZapTypeDesc zap_desc_thread = {(int64_t)sizeof(ZapThread), ZAP_TD_ATOMIC, 0, NULL};

void *zap_thread_new(void) {
    ZapThread *t = (ZapThread *)zap_obj(&zap_desc_thread);
    pthread_mutex_init(&t->mu, NULL);
    t->started = 0;
    t->done = 0;
    t->wq_head = t->wq_tail = NULL;
    return t;
}

/* Called synchronously inside `Thread.start`, before the coroutine is spawned,
 * so a non-daemon thread is counted in `g_live` with no start race. */
void zap_thread_register(void *tp, int64_t daemon) {
    ZapThread *t = (ZapThread *)tp;
    zap_np_lock(&t->mu);
    t->started = 1;
    t->done = 0;
    zap_np_unlock(&t->mu);
    if (!daemon) {
        zap_np_lock(&g_live_mu);
        g_live++;
        zap_np_unlock(&g_live_mu);
    }
}

/* Called from the thread body's `finally`: publish completion (waking joiners)
 * and drop a non-daemon thread's `g_live` reference. */
void zap_thread_finish(void *tp, int64_t daemon) {
    ZapThread *t = (ZapThread *)tp;
    zap_np_lock(&t->mu);
    t->done = 1;
    Waiter *x;
    while ((x = q_pop(&t->wq_head, &t->wq_tail)) != NULL) {
        coready(x);
    }
    zap_np_unlock(&t->mu);
    if (!daemon) {
        zap_np_lock(&g_live_mu);
        if (--g_live == 0) {
            pthread_cond_broadcast(&g_live_cv);
        }
        zap_np_unlock(&g_live_mu);
    }
}

void zap_thread_join(void *tp) {
    ZapThread *t = (ZapThread *)tp;
    zap_np_lock(&t->mu);
    if (t->done) {
        zap_np_unlock(&t->mu);
        return;
    }
    Waiter w;
    w.g = NULL;
    q_push(&t->wq_head, &t->wq_tail, &w);
    copark(&t->mu, &w); /* releases t->mu; woken by zap_thread_finish */
}

/* Join with a timeout (seconds). 1 if the thread finished, 0 on timeout. */
int64_t zap_thread_join_timeout(void *tp, double secs) {
    ZapThread *t = (ZapThread *)tp;
    zap_np_lock(&t->mu);
    if (t->done) {
        zap_np_unlock(&t->mu);
        return 1;
    }
    Waiter w;
    w.g = NULL;
    q_push(&t->wq_head, &t->wq_tail, &w);
    int woken = copark_timed(&t->mu, &w, secs); /* returns with t->mu held */
    if (!woken) {
        q_remove(&t->wq_head, &t->wq_tail, &w);
    }
    zap_np_unlock(&t->mu);
    return woken ? 1 : 0;
}

int64_t zap_thread_is_alive(void *tp) {
    ZapThread *t = (ZapThread *)tp;
    zap_np_lock(&t->mu);
    int64_t alive = (t->started && !t->done) ? 1 : 0;
    zap_np_unlock(&t->mu);
    return alive;
}

/* ---- Lock (non-reentrant) and timed acquire -------------------------------
 * Reuses the `sync.Mutex` runtime (direct handoff on unlock). The timed acquire
 * is handoff-compatible: a woken waiter observes `w.done` under the lock, so a
 * timeout that races the handoff still returns "acquired" rather than losing the
 * lock. */
int64_t zap_mutex_lock_timeout(void *mp, double secs) {
    ZapMutex *m = (ZapMutex *)mp;
    zap_np_lock(&m->mu);
    if (!m->locked) {
        m->locked = 1;
        zap_np_unlock(&m->mu);
        return 1;
    }
    Waiter w;
    w.g = NULL;
    q_push(&m->wq_head, &m->wq_tail, &w);
    int woken = copark_timed(&m->mu, &w, secs);
    if (!woken) {
        q_remove(&m->wq_head, &m->wq_tail, &w);
    }
    zap_np_unlock(&m->mu);
    return woken ? 1 : 0; /* woken => the lock was handed to us */
}

/* ---- RLock: reentrant lock, owner-tracked ---------------------------------
 * The owner identity is the current coroutine's G (stable across a pinned
 * Thread's parks/migrations), or the OS thread for a non-coroutine (main). */
typedef struct {
    pthread_mutex_t mu;
    void *owner;               /* current holder id, or NULL if unheld          */
    int64_t count;             /* recursion depth                               */
    Waiter *wq_head, *wq_tail;
} ZapRLock;

static const ZapTypeDesc zap_desc_rlock = {(int64_t)sizeof(ZapRLock), ZAP_TD_ATOMIC, 0, NULL};

/* Reserved `owner` value during an unlock->wake handoff: the lock is not free
 * (racers, including the just-released holder, must park) but not yet claimed by
 * the woken waiter, which overwrites it with its own id. Never a real id (G
 * pointers are heap addresses; a thread id is never 1). */
#define ZAP_RLOCK_HANDOFF ((void *)(uintptr_t)1)

static void *thr_self_id(void) {
    M *m = cur_m;
    if (m && m->current) {
        return (void *)m->current; /* the coroutine (pinned Thread or ordinary) */
    }
    return (void *)(uintptr_t)pthread_self(); /* main / non-coroutine thread */
}

void *zap_rlock_new(void) {
    ZapRLock *r = (ZapRLock *)zap_obj(&zap_desc_rlock);
    pthread_mutex_init(&r->mu, NULL);
    r->owner = NULL;
    r->count = 0;
    r->wq_head = r->wq_tail = NULL;
    return r;
}

void zap_rlock_lock(void *rp) {
    ZapRLock *r = (ZapRLock *)rp;
    void *self = thr_self_id();
    zap_np_lock(&r->mu);
    if (r->owner == self) {
        r->count++;
        zap_np_unlock(&r->mu);
        return;
    }
    if (r->owner == NULL) {
        r->owner = self;
        r->count = 1;
        zap_np_unlock(&r->mu);
        return;
    }
    Waiter w;
    w.g = NULL;
    q_push(&r->wq_head, &r->wq_tail, &w);
    copark(&r->mu, &w);
    /* Woken by a direct handoff from zap_rlock_unlock: owner is the HANDOFF
     * sentinel and count is already 1; claim it by stamping our id. */
    zap_np_lock(&r->mu);
    r->owner = self;
    zap_np_unlock(&r->mu);
}

int64_t zap_rlock_trylock(void *rp) {
    ZapRLock *r = (ZapRLock *)rp;
    void *self = thr_self_id();
    zap_np_lock(&r->mu);
    int64_t got = 0;
    if (r->owner == self) {
        r->count++;
        got = 1;
    } else if (r->owner == NULL) {
        r->owner = self;
        r->count = 1;
        got = 1;
    }
    zap_np_unlock(&r->mu);
    return got;
}

void zap_rlock_unlock(void *rp) {
    ZapRLock *r = (ZapRLock *)rp;
    void *self = thr_self_id();
    zap_np_lock(&r->mu);
    if (r->owner != self || r->count <= 0) {
        zap_np_unlock(&r->mu);
        zap_panic(zap_str_new("cannot release un-acquired RLock", 32));
        return;
    }
    if (--r->count > 0) {
        zap_np_unlock(&r->mu);
        return; /* still held by this owner */
    }
    Waiter *nx = q_pop(&r->wq_head, &r->wq_tail);
    if (nx) {
        /* Hand off: reserve ownership (count stays 1) so no racer can slip in
         * before the woken waiter stamps its own id. */
        r->owner = ZAP_RLOCK_HANDOFF;
        r->count = 1;
        coready(nx);
    } else {
        r->owner = NULL;
        r->count = 0;
    }
    zap_np_unlock(&r->mu);
}

/* ---- Event: a latch; wait blocks until set --------------------------------- */
typedef struct {
    pthread_mutex_t mu;
    int is_set;
    Waiter *wq_head, *wq_tail;
} ZapEvent;

static const ZapTypeDesc zap_desc_event = {(int64_t)sizeof(ZapEvent), ZAP_TD_ATOMIC, 0, NULL};

void *zap_event_new(void) {
    ZapEvent *e = (ZapEvent *)zap_obj(&zap_desc_event);
    pthread_mutex_init(&e->mu, NULL);
    e->is_set = 0;
    e->wq_head = e->wq_tail = NULL;
    return e;
}

void zap_event_set(void *ep) {
    ZapEvent *e = (ZapEvent *)ep;
    zap_np_lock(&e->mu);
    e->is_set = 1;
    Waiter *x;
    while ((x = q_pop(&e->wq_head, &e->wq_tail)) != NULL) {
        coready(x);
    }
    zap_np_unlock(&e->mu);
}

void zap_event_clear(void *ep) {
    ZapEvent *e = (ZapEvent *)ep;
    zap_np_lock(&e->mu);
    e->is_set = 0;
    zap_np_unlock(&e->mu);
}

int64_t zap_event_is_set(void *ep) {
    ZapEvent *e = (ZapEvent *)ep;
    zap_np_lock(&e->mu);
    int64_t s = e->is_set ? 1 : 0;
    zap_np_unlock(&e->mu);
    return s;
}

void zap_event_wait(void *ep) {
    ZapEvent *e = (ZapEvent *)ep;
    zap_np_lock(&e->mu);
    if (e->is_set) {
        zap_np_unlock(&e->mu);
        return;
    }
    Waiter w;
    w.g = NULL;
    q_push(&e->wq_head, &e->wq_tail, &w);
    copark(&e->mu, &w);
}

int64_t zap_event_wait_timeout(void *ep, double secs) {
    ZapEvent *e = (ZapEvent *)ep;
    zap_np_lock(&e->mu);
    if (e->is_set) {
        zap_np_unlock(&e->mu);
        return 1;
    }
    Waiter w;
    w.g = NULL;
    q_push(&e->wq_head, &e->wq_tail, &w);
    int woken = copark_timed(&e->mu, &w, secs);
    if (!woken) {
        q_remove(&e->wq_head, &e->wq_tail, &w);
    }
    zap_np_unlock(&e->mu);
    return woken ? 1 : 0;
}

/* ---- Semaphore: counting ---------------------------------------------------
 * `release` hands a permit directly to a parked waiter (count stays spent), so
 * a woken `acquire` returns owning a permit with no re-check. */
typedef struct {
    pthread_mutex_t mu;
    int64_t count;
    Waiter *wq_head, *wq_tail;
} ZapSem;

static const ZapTypeDesc zap_desc_sem = {(int64_t)sizeof(ZapSem), ZAP_TD_ATOMIC, 0, NULL};

void *zap_sem_new(int64_t n) {
    ZapSem *s = (ZapSem *)zap_obj(&zap_desc_sem);
    pthread_mutex_init(&s->mu, NULL);
    s->count = n < 0 ? 0 : n;
    s->wq_head = s->wq_tail = NULL;
    return s;
}

void zap_sem_acquire(void *sp) {
    ZapSem *s = (ZapSem *)sp;
    zap_np_lock(&s->mu);
    if (s->count > 0) {
        s->count--;
        zap_np_unlock(&s->mu);
        return;
    }
    Waiter w;
    w.g = NULL;
    q_push(&s->wq_head, &s->wq_tail, &w);
    copark(&s->mu, &w); /* woken by a direct permit handoff */
}

int64_t zap_sem_acquire_timeout(void *sp, double secs) {
    ZapSem *s = (ZapSem *)sp;
    zap_np_lock(&s->mu);
    if (s->count > 0) {
        s->count--;
        zap_np_unlock(&s->mu);
        return 1;
    }
    Waiter w;
    w.g = NULL;
    q_push(&s->wq_head, &s->wq_tail, &w);
    int woken = copark_timed(&s->mu, &w, secs);
    if (!woken) {
        q_remove(&s->wq_head, &s->wq_tail, &w);
    }
    zap_np_unlock(&s->mu);
    return woken ? 1 : 0; /* woken => a permit was handed to us */
}

void zap_sem_release(void *sp) {
    ZapSem *s = (ZapSem *)sp;
    zap_np_lock(&s->mu);
    Waiter *nx = q_pop(&s->wq_head, &s->wq_tail);
    if (nx) {
        coready(nx); /* hand the permit straight to a waiter; count unchanged */
    } else {
        s->count++;
    }
    zap_np_unlock(&s->mu);
}

/* ---- Condition: own internal lock (matches Python's default Condition()) ---
 * `acquire`/`release` take the condition lock; `wait` atomically releases it,
 * parks, and reacquires on wake; `notify`/`notify_all` move waiters from the
 * condition queue to the lock queue. The internal lock is a non-reentrant
 * Mutex-style lock (documented; Python's default wraps an RLock). */
typedef struct {
    pthread_mutex_t mu;
    int locked;                /* condition lock held                           */
    Waiter *lq_head, *lq_tail; /* waiters for the lock                          */
    Waiter *cq_head, *cq_tail; /* waiters in wait(), awaiting notify            */
} ZapCond;

static const ZapTypeDesc zap_desc_cond = {(int64_t)sizeof(ZapCond), ZAP_TD_ATOMIC, 0, NULL};

void *zap_cond_new(void) {
    ZapCond *c = (ZapCond *)zap_obj(&zap_desc_cond);
    pthread_mutex_init(&c->mu, NULL);
    c->locked = 0;
    c->lq_head = c->lq_tail = NULL;
    c->cq_head = c->cq_tail = NULL;
    return c;
}

void zap_cond_acquire(void *cp) {
    ZapCond *c = (ZapCond *)cp;
    zap_np_lock(&c->mu);
    if (!c->locked) {
        c->locked = 1;
        zap_np_unlock(&c->mu);
        return;
    }
    Waiter w;
    w.g = NULL;
    q_push(&c->lq_head, &c->lq_tail, &w);
    copark(&c->mu, &w); /* direct handoff: we own the lock on wake */
}

/* Release the condition lock: hand off to a lock-waiter, else mark unlocked.
 * Caller must hold c->mu. */
static void cond_release_locked(ZapCond *c) {
    Waiter *nx = q_pop(&c->lq_head, &c->lq_tail);
    if (nx) {
        coready(nx); /* ownership passes; locked stays 1 */
    } else {
        c->locked = 0;
    }
}

void zap_cond_release(void *cp) {
    ZapCond *c = (ZapCond *)cp;
    zap_np_lock(&c->mu);
    cond_release_locked(c);
    zap_np_unlock(&c->mu);
}

void zap_cond_wait(void *cp) {
    ZapCond *c = (ZapCond *)cp;
    zap_np_lock(&c->mu);
    /* Release the lock and park on the condition queue atomically under c->mu. */
    cond_release_locked(c);
    Waiter w;
    w.g = NULL;
    q_push(&c->cq_head, &c->cq_tail, &w);
    copark(&c->mu, &w);
    /* notify moved us onto the lock queue and a handoff woke us: we own the
     * lock again on return. */
}

/* Move up to `n` condition-waiters onto the lock queue (they reacquire via the
 * handoff chain). notify_all uses n < 0. */
static void cond_notify_n(ZapCond *c, int64_t n) {
    zap_np_lock(&c->mu);
    while (n != 0) {
        Waiter *x = q_pop(&c->cq_head, &c->cq_tail);
        if (!x) {
            break;
        }
        if (!c->locked) {
            /* Lock free: hand it straight to this waiter. */
            c->locked = 1;
            coready(x);
        } else {
            /* Lock held: queue the waiter for the lock's handoff chain. */
            q_push(&c->lq_head, &c->lq_tail, x);
        }
        if (n > 0) {
            n--;
        }
    }
    zap_np_unlock(&c->mu);
}

void zap_cond_notify(void *cp, int64_t n) { cond_notify_n((ZapCond *)cp, n); }
void zap_cond_notify_all(void *cp) { cond_notify_n((ZapCond *)cp, -1); }
