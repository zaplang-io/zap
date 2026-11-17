/* zap runtime fragment: profile.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * CPU profiler (ticket #78, D-0058)
 * ---------------------------------------------------------------------------
 * A sampling CPU profiler built on the existing signal + scheduler runtime.
 * Design: design/0014-profiling.md.
 *
 *   TRIGGER   `SIGUSR2` toggles profiling (D-0058); it is routed through the
 *             dedicated signal thread into zap_sig_run_default -> here, so
 *             start/stop run in NORMAL context (may allocate, open the output
 *             file, start the monitor thread, symbolicate at stop). `ZAP_PROFILE
 *             =cpu` auto-starts a whole-run profile; `runtime.profile_start/stop`
 *             bracket a region. `ZAP_PROFILE_HZ` (default 100) and
 *             `ZAP_PROFILE_OUT` (default `zap-cpuprofile-<pid>.folded`, or `-`
 *             for stderr) configure it.
 *
 *   SAMPLING  A dedicated monitor thread (modeled on the #172 preemption
 *             monitor) sleeps one sample period and `pthread_kill`s SIGPROF to
 *             the on-CPU coroutine of every worker M across all Ps — the only
 *             way to sample all Ps in the M:N scheduler (a process-directed
 *             timer hits one arbitrary thread). SIGPROF is reserved by the
 *             runtime exactly as SIGURG is for preemption; it is NOT a member of
 *             the `Signal` prelude enum, so `signal.notify` cannot claim it.
 *
 *   HANDLER   Runs on the target worker, strictly async-signal-safe: it reuses
 *             the #172 preempt-handler guard (interrupted SP must be inside the
 *             running coroutine's green stack; no runtime lock/allocation held;
 *             not mid-GC/STW; on-CPU, not blocked) and then `backtrace()`s into
 *             an on-stack buffer and pushes the raw return addresses into a
 *             PREALLOCATED per-M lock-free ring. It never allocates, locks, or
 *             symbolicates. A sample taken while the coroutine is in the runtime
 *             is counted in a synthetic `[runtime]` bucket rather than dropped,
 *             so the profile accounts for all on-CPU time.
 *
 *   GC-SAFE   A sample records only return addresses (immovable code), never
 *             heap object pointers, so it is immune to the #120 moving collector
 *             relocating objects; a worker parked at a GC safepoint is off-CPU
 *             and so is simply not sampled.
 *
 *   OUTPUT    Dependency-free folded/collapsed stacks (`frame;frame;frame N`),
 *             symbolicated OFFLINE at stop via backtrace_symbols (the link adds
 *             -rdynamic so Zap function names resolve). A pprof-protobuf writer
 *             is a follow-up, done offline in the Rust driver, not in this
 *             async-safe path. Allocation profiling is a v2 follow-up.
 *
 * Linux/glibc only in v1 (needs backtrace() + the netpoller-gated signal
 * thread); a no-op otherwise, consistent with the rest of the signal subsystem.
 * =========================================================================== */
#define ZAP_PROF_MAX_DEPTH 64   /* frames captured per sample                    */
#define ZAP_PROF_RING_CAP 8192  /* samples retained per worker M (oldest drop)   */

typedef struct {
    int32_t coro;  /* G.all_idx of the sampled coroutine                         */
    int32_t depth; /* number of PCs in `pcs` (innermost first, backtrace order)  */
    void *pcs[ZAP_PROF_MAX_DEPTH];
} ZapProfSample;

typedef struct {
    ZapProfSample *buf;     /* ZAP_PROF_RING_CAP slots, preallocated              */
    volatile uint64_t head; /* monotonically increasing; slot = head % cap        */
} ZapProfRing;

static pthread_mutex_t g_prof_mu = PTHREAD_MUTEX_INITIALIZER; /* start/stop guard */
static ZapProfRing *g_prof_rings = NULL; /* one per worker M (g_nm); process-life */
static int g_prof_rings_n = 0;
static volatile int g_prof_active = 0;    /* handlers write only while set        */
static volatile int g_prof_stop = 0;      /* monitor exit flag                    */
static volatile uint64_t g_prof_runtime_samples = 0; /* [runtime] bucket (atomic) */
static pthread_t g_prof_monitor;
static int g_prof_monitor_started = 0;
static int g_prof_env_requested = 0;      /* ZAP_PROFILE=cpu seen at startup      */
static long g_prof_hz = 100;              /* sample rate, resolved at start       */
static char g_prof_out_path[1024];        /* resolved output path at start        */

#ifdef ZAP_HAVE_BACKTRACE

/* SA_RESTART handler installed once, process-wide. SA_RESTART auto-resumes the
 * restartable syscalls (read/write/recv/send/accept/connect); the few that it
 * does not (poll/epoll_wait/nanosleep) are EINTR-looped at their call sites. No
 * SA_ONSTACK: the handler must run on the coroutine's own green stack so the
 * green-stack-range guard below is meaningful. */
static void zap_prof_handler(int sig, siginfo_t *info, void *uc) {
    (void)sig;
    (void)info;
    (void)uc;
    if (!g_prof_active) {
        return;
    }
    M *m = cur_m;
    if (!m || !m->current) {
        return; /* scheduler/idle/native thread: nothing on-CPU to sample */
    }
    if (m < g_ms || m >= g_ms + g_nm) {
        return; /* dedicated (LockOSThread / #175) or aux M: no ring, don't sample */
    }
    G *g = m->current;
    if (g->locked_m) {
        return; /* pinned coroutine: excluded, like preemption */
    }
#ifdef ZAP_GC_IMMIX
    /* Off-CPU (blocked in a syscall / parked for GC) or mid-collection: not a CPU
     * sample. zap_self is this thread's own mutator node, safe to read here. */
    if (zap_sp_test(ZAP_SP_COLLECTING | ZAP_SP_GC_WANTED)) {
        return;
    }
    if (zap_self && zap_self->state != ZAP_MUT_ACTIVE) {
        return;
    }
#else
    if (__atomic_load_n(&g_boehm_in_stw, __ATOMIC_SEQ_CST)) {
        return; /* Boehm is stopping the world; stay out of its way (seq-cst
                 * acquire, paired with the release store in zap_boehm_gc_event —
                 * #693/#213, D-0120) */
    }
#endif
    /* In a runtime critical section (holds a lock / mid-allocation): bucket the
     * sample as [runtime] rather than unwind through inconsistent runtime state. */
    if (g->no_preempt != 0) {
        __atomic_fetch_add(&g_prof_runtime_samples, 1, __ATOMIC_RELAXED);
        return;
    }
    /* The signal can land while the worker is on its NATIVE scheduler stack
     * mid context-switch (m->current set, context not yet swapped). Unwinding
     * there is meaningless/unsafe: require the interrupted SP to be inside the
     * running coroutine's green stack, exactly as the #172 preempt handler does. */
    char probe;
    char *sp = &probe;
    char *lo = (char *)g->stack;
    char *hi = lo + g->stack_size;
    if (!(sp >= lo && sp < hi)) {
        __atomic_fetch_add(&g_prof_runtime_samples, 1, __ATOMIC_RELAXED);
        return;
    }
    void *frames[ZAP_PROF_MAX_DEPTH];
    int n = backtrace(frames, ZAP_PROF_MAX_DEPTH); /* async-safe (warmed at start) */
    if (n <= 0) {
        return;
    }
    int idx = (int)(m - g_ms);
    ZapProfRing *r = &g_prof_rings[idx];
    uint64_t h = r->head; /* single producer (this worker): no CAS needed */
    ZapProfSample *s = &r->buf[h % ZAP_PROF_RING_CAP];
    s->coro = (int32_t)g->all_idx;
    int d = n < ZAP_PROF_MAX_DEPTH ? n : ZAP_PROF_MAX_DEPTH;
    for (int i = 0; i < d; i++) {
        s->pcs[i] = frames[i];
    }
    s->depth = d;
    __atomic_store_n(&r->head, h + 1, __ATOMIC_RELEASE);
}

/* The sampling monitor: a plain OS thread (never touches the GC heap, holds no
 * lock a collection needs, and blocks the signals it sends on itself). */
static void *zap_prof_monitor_main(void *arg) {
    (void)arg;
    sigset_t block;
    sigemptyset(&block);
    sigaddset(&block, SIGPROF);
    sigaddset(&block, SIGURG);
    pthread_sigmask(SIG_BLOCK, &block, NULL);
    long hz = g_prof_hz;
    if (hz < 1) {
        hz = 1;
    }
    long period_ns = 1000000000L / hz;
    while (!g_prof_stop) {
        struct timespec ts;
        ts.tv_sec = period_ns / 1000000000L;
        ts.tv_nsec = period_ns % 1000000000L;
        nanosleep(&ts, NULL); /* a signal cutting this short only samples sooner */
        if (g_prof_stop) {
            break;
        }
        for (int i = 0; i < g_nm; i++) {
            M *m = &g_ms[i];
            /* Benign lock-free read, like the preemption monitor: `current` is
             * only tested for non-NULL and the handler re-checks on the worker. */
            if (m->current) {
                pthread_kill(m->thread, SIGPROF);
            }
        }
    }
    return NULL;
}

static void zap_prof_install_once(void) {
    static int installed = 0;
    if (installed) {
        return;
    }
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = zap_prof_handler;
    sa.sa_flags = SA_SIGINFO | SA_RESTART;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGPROF, &sa, NULL) == 0) {
        installed = 1;
    }
}

static void zap_prof_resolve_config(void) {
    g_prof_hz = 100;
    const char *hz = getenv("ZAP_PROFILE_HZ");
    if (hz && hz[0]) {
        long v = atol(hz);
        if (v >= 1 && v <= 100000) {
            g_prof_hz = v;
        }
    }
    const char *out = getenv("ZAP_PROFILE_OUT");
    if (out && out[0]) {
        snprintf(g_prof_out_path, sizeof g_prof_out_path, "%s", out);
    } else {
        snprintf(g_prof_out_path, sizeof g_prof_out_path, "zap-cpuprofile-%ld.folded",
                 (long)getpid());
    }
}

/* ---- folded-stack output (stop path, normal context: may malloc) ---------- */
/* Extract a short frame name from a glibc backtrace_symbols() string, which is
 * "module(symbol+0xoff) [0xaddr]" (symbol present only with -rdynamic). Prefer
 * the symbol; else the "module+0xoff" form; else the raw string. Writes into
 * `dst` (NUL-terminated, truncated to cap). */
static void zap_prof_frame_name(const char *sym, char *dst, size_t cap) {
    if (!sym || cap == 0) {
        if (cap) {
            dst[0] = '\0';
        }
        return;
    }
    const char *open = strchr(sym, '(');
    if (open) {
        const char *name = open + 1;
        const char *plus = strchr(name, '+');
        const char *close = strchr(name, ')');
        const char *end = plus ? plus : close;
        if (end && end > name) {
            size_t len = (size_t)(end - name);
            if (len >= cap) {
                len = cap - 1;
            }
            memcpy(dst, name, len);
            dst[len] = '\0';
            return;
        }
        /* "module(+0xoff)" or "module()" — no symbol name; emit just the module
         * path (up to the '('), so an unresolved frame reads cleanly without a
         * dangling parenthesis. */
        size_t len = (size_t)(open - sym);
        if (len >= cap) {
            len = cap - 1;
        }
        memcpy(dst, sym, len);
        dst[len] = '\0';
        return;
    }
    snprintf(dst, cap, "%s", sym);
}

/* A tallied folded stack (one unique "a;b;c" string + its sample count). */
typedef struct {
    char *key;
    uint64_t count;
} ZapFolded;

static void zap_prof_write_folded(void) {
    ZapFolded *tab = NULL;
    size_t ntab = 0, captab = 0;
    uint64_t total = 0, dropped = 0;

    for (int i = 0; i < g_prof_rings_n; i++) {
        ZapProfRing *r = &g_prof_rings[i];
        uint64_t head = r->head;
        uint64_t start = head > ZAP_PROF_RING_CAP ? head - ZAP_PROF_RING_CAP : 0;
        if (head > ZAP_PROF_RING_CAP) {
            dropped += head - ZAP_PROF_RING_CAP;
        }
        for (uint64_t h = start; h < head; h++) {
            ZapProfSample *s = &r->buf[h % ZAP_PROF_RING_CAP];
            int d = s->depth;
            if (d <= 0) {
                continue;
            }
            char **syms = backtrace_symbols(s->pcs, d);
            /* Build "root;...;leaf": backtrace is innermost-first, so emit in
             * reverse so the folded line reads caller -> callee (flamegraph). */
            char line[4096];
            size_t off = 0;
            line[0] = '\0';
            for (int k = d - 1; k >= 0; k--) {
                char name[256];
                if (syms) {
                    zap_prof_frame_name(syms[k], name, sizeof name);
                } else {
                    snprintf(name, sizeof name, "%p", s->pcs[k]);
                }
                int w = snprintf(line + off, sizeof line - off, "%s%s",
                                 off ? ";" : "", name);
                if (w < 0) {
                    break;
                }
                off += (size_t)w;
                if (off >= sizeof line) {
                    off = sizeof line - 1;
                    break;
                }
            }
            if (syms) {
                free(syms);
            }
            total++;
            /* Tally into the folded table (linear scan: few unique stacks). */
            size_t j = 0;
            for (; j < ntab; j++) {
                if (strcmp(tab[j].key, line) == 0) {
                    tab[j].count++;
                    break;
                }
            }
            if (j == ntab) {
                if (ntab == captab) {
                    size_t nc = captab ? captab * 2 : 32;
                    ZapFolded *nt = (ZapFolded *)realloc(tab, nc * sizeof(ZapFolded));
                    if (!nt) {
                        break;
                    }
                    tab = nt;
                    captab = nc;
                }
                tab[ntab].key = strdup(line);
                tab[ntab].count = 1;
                ntab++;
            }
        }
    }

    uint64_t rt = __atomic_load_n(&g_prof_runtime_samples, __ATOMIC_RELAXED);
    FILE *f = NULL;
    int to_stderr = (strcmp(g_prof_out_path, "-") == 0);
    if (to_stderr) {
        f = stderr;
    } else {
        f = fopen(g_prof_out_path, "w");
    }
    if (f) {
        for (size_t j = 0; j < ntab; j++) {
            fprintf(f, "%s %llu\n", tab[j].key, (unsigned long long)tab[j].count);
        }
        if (rt > 0) {
            fprintf(f, "[runtime] %llu\n", (unsigned long long)rt);
        }
        if (!to_stderr) {
            fclose(f);
        } else {
            fflush(f);
        }
    }
    for (size_t j = 0; j < ntab; j++) {
        free(tab[j].key);
    }
    free(tab);

    /* Deterministic banner (no sample counts in the matched prefix) so a golden
     * can assert the profiler ran; the detail after is informational. */
    fprintf(stderr,
            "=== zap CPU profile ===\n"
            "samples=%llu runtime=%llu dropped=%llu hz=%ld -> %s\n",
            (unsigned long long)total, (unsigned long long)rt,
            (unsigned long long)dropped, g_prof_hz,
            to_stderr ? "(stderr)" : g_prof_out_path);
    fflush(stderr);
}

static void zap_prof_warmup(void) {
    void *f[8];
    int n = backtrace(f, 8); /* force libgcc's lazy unwinder init off the handler */
    (void)n;
    char **s = backtrace_symbols(f, n < 1 ? 1 : n);
    if (s) {
        free(s); /* warm backtrace_symbols' lazy dlopen too (used only at stop) */
    }
}

static void zap_profile_start_locked(void) {
    if (g_prof_active) {
        return;
    }
    if (!g_ms || g_nm <= 0) {
        fprintf(stderr, "zap: profile: no scheduler yet (spawn a coroutine first)\n");
        return;
    }
    zap_prof_resolve_config();
    if (!g_prof_rings) {
        g_prof_rings = (ZapProfRing *)calloc((size_t)g_nm, sizeof(ZapProfRing));
        if (!g_prof_rings) {
            fprintf(stderr, "zap: profile: out of memory\n");
            return;
        }
        for (int i = 0; i < g_nm; i++) {
            g_prof_rings[i].buf =
                (ZapProfSample *)calloc(ZAP_PROF_RING_CAP, sizeof(ZapProfSample));
            if (!g_prof_rings[i].buf) {
                fprintf(stderr, "zap: profile: out of memory\n");
                for (int j = 0; j < i; j++) {
                    free(g_prof_rings[j].buf);
                }
                free(g_prof_rings);
                g_prof_rings = NULL;
                return;
            }
        }
        g_prof_rings_n = g_nm;
    }
    /* Reset counters for this run (rings are reused across toggles). */
    for (int i = 0; i < g_prof_rings_n; i++) {
        g_prof_rings[i].head = 0;
    }
    __atomic_store_n(&g_prof_runtime_samples, 0, __ATOMIC_RELAXED);
    zap_prof_install_once();
    zap_prof_warmup();
    g_prof_stop = 0;
    __atomic_store_n(&g_prof_active, 1, __ATOMIC_RELEASE);
    if (zap_gc_pthread_create(&g_prof_monitor, NULL, zap_prof_monitor_main, NULL) == 0) {
        g_prof_monitor_started = 1;
    } else {
        __atomic_store_n(&g_prof_active, 0, __ATOMIC_RELEASE);
        fprintf(stderr, "zap: profile: monitor thread start failed\n");
    }
}

static void zap_profile_stop_locked(void) {
    if (!g_prof_active) {
        return;
    }
    /* Stop writes first, then the monitor; after the join no new SIGPROF is sent
     * and any handler now sees g_prof_active == 0 and skips. The rings persist
     * (freed only at zap_profile_teardown, after all workers are joined), so a
     * straggler handler can never write into freed memory. */
    __atomic_store_n(&g_prof_active, 0, __ATOMIC_RELEASE);
    g_prof_stop = 1;
    if (g_prof_monitor_started) {
        zap_gc_pthread_join(g_prof_monitor, NULL);
        g_prof_monitor_started = 0;
    }
    zap_prof_write_folded();
}

#else /* !ZAP_HAVE_BACKTRACE: profiling unavailable (non-glibc) */
static void zap_profile_start_locked(void) {
    fprintf(stderr, "zap: profile: unsupported on this platform (no backtrace)\n");
}
static void zap_profile_stop_locked(void) {}
#endif /* ZAP_HAVE_BACKTRACE */

/* ---- public ABI ----------------------------------------------------------- */
void zap_profile_start(void) {
    zap_np_lock(&g_prof_mu);
    zap_profile_start_locked();
    zap_np_unlock(&g_prof_mu);
}

void zap_profile_stop(void) {
    zap_np_lock(&g_prof_mu);
    zap_profile_stop_locked();
    zap_np_unlock(&g_prof_mu);
}

void zap_profile_toggle(void) {
    zap_np_lock(&g_prof_mu);
    if (g_prof_active) {
        zap_profile_stop_locked();
    } else {
        zap_profile_start_locked();
    }
    zap_np_unlock(&g_prof_mu);
}

static void zap_profile_env_init(void) {
    const char *p = getenv("ZAP_PROFILE");
    if (p && (strcmp(p, "cpu") == 0 || strcmp(p, "1") == 0 || strcmp(p, "on") == 0)) {
        g_prof_env_requested = 1;
    }
}

static void zap_profile_env_maybe_start(void) {
    if (g_prof_env_requested) {
        zap_profile_start();
    }
}

static void zap_profile_teardown(void) {
#ifdef ZAP_HAVE_BACKTRACE
    if (g_prof_rings) {
        for (int i = 0; i < g_prof_rings_n; i++) {
            free(g_prof_rings[i].buf);
        }
        free(g_prof_rings);
        g_prof_rings = NULL;
        g_prof_rings_n = 0;
    }
#endif
}
