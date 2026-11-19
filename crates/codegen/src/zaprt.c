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
/* ===========================================================================
 * Platform detection (ticket #199, platform epic #158 / Area A5). The runtime
 * is POSIX/Linux-x86_64 today; Windows support is being built out across the
 * Area-A sub-tickets. This block classifies the target and, on Windows, fails
 * the compile with an authoritative, up-to-date map of the remaining gaps so
 * the port is never *silently* broken — the A5 toolchain (this ticket) wires
 * the build/link path and the A1 context-switch seam below, but the runtime
 * itself cannot fully compile for Windows until A1/A2/A3/A4 land.
 *
 * Note: this `zaprt.c` is compiled by `cc`/MinGW only at *program link* time
 * (see build_executable in lib.rs); it is NOT compiled by `cargo build` of the
 * compiler crates. With A1/A2/A3/A4/A5 and now the Immix GC port (I#218) landed,
 * it compiles and links on Windows under BOTH collectors; the `windows-runtime`
 * CI job proves this by building + running the integration harness under each. */
#if defined(_WIN32)
#define ZAP_PLATFORM_WINDOWS 1
#else
#define ZAP_PLATFORM_POSIX 1
#endif

/* Windows now builds under BOTH collectors. gc-boehm (the conservative fallback)
 * landed with A5/A1/A2/A3/A4; the precise Immix collector — Zap's DEFAULT — is
 * ported to Windows by I#218 (I#158/A-GC): its aligned-block heap in
 * zapgc.inc.c uses `_aligned_malloc`/`_aligned_free` on Windows (the seam there;
 * the allocator never used mmap, so there is no VirtualAlloc reserve/commit
 * layer to carry). The former `#error` that blocked ZAP_GC_IMMIX on Windows is
 * therefore retired. See design/0016-platform-1.0.md. */

/* Python.h must precede the standard headers (it sets feature-test macros).
 * Embedding CPython backs the Python interop (ticket #27, D-0012); the compile
 * flags come from `pkg-config --cflags python3-embed` (see build_executable),
 * and the dev container / CI provide `python3-dev`. (Windows v1 gates CPython
 * off — see build_executable — so this is a POSIX-era dependency; the Windows
 * gate above halts the compile before here regardless.) */
/* CPython embedding is gated OFF for Windows v1 (I#198): there is no reliable
 * `python3-embed.pc` under MinGW-w64, so build_executable omits the CPython
 * cflags/libs on Windows and the `import python` entry points below become
 * catchable-error stubs. Guard the header so the TU compiles without Python.h. */
/* CPython embedding is enabled on POSIX, UNLESS the portable build profile is
 * selected (`zap build --portable` / `ZAP_PORTABLE=1`, I#287): that profile
 * drops libpython so the program is self-contained and statically linkable, and
 * runs on minimal / older-glibc distros such as Amazon Linux 2023 (the AWS
 * `provided.al2023` Lambda base). build_executable then passes `-DZAP_NO_CPYTHON`
 * and omits the `pkg-config python3-embed` cflags/libs, so the `import python`
 * entry points below become the same catchable-error stubs Windows v1 (I#198)
 * uses. Guard the header so the TU still compiles without Python.h. */
#if defined(ZAP_PLATFORM_POSIX) && !defined(ZAP_NO_CPYTHON)
#define ZAP_HAVE_CPYTHON 1
#define PY_SSIZE_T_CLEAN
#include <Python.h>
#endif

/* `NULL` / `size_t` for the code between here and the standard-header block
 * below (e.g. the precise-Immix `GC_get_my_stackbottom` shim). When CPython is
 * embedded, `<Python.h>` above already pulls these in; when it is NOT — the
 * portable profile (`-DZAP_NO_CPYTHON`, I#287) or Windows v1 (I#198) — nothing
 * has yet, so include the freestanding `<stddef.h>` explicitly. It defines no
 * feature-test-gated declarations, so it is safe before the other headers (the
 * "Python.h must precede the standard headers" ordering above is unaffected). */
#include <stddef.h>

#ifndef ZAP_GC_IMMIX
/* `GC_THREADS` must be defined before <gc.h> so the collector is built/used in
 * its thread-aware mode (ticket #25, D-0009/D-0013): it then knows about every
 * coroutine OS thread and scans each one's stack and registers, so live objects
 * reachable only from a running or suspended coroutine are never collected. The
 * coroutine runtime starts threads with `GC_pthread_create` (declared by <gc.h>
 * in this mode) so the collector registers them automatically; channels block
 * on pthread mutex/condvars. The dev container's `libgc-dev` has thread
 * support, and the program links `-lpthread` (see build_executable). */
#define GC_THREADS
#if defined(_WIN32)
/* On Windows, `GC_THREADS` alone selects bdwgc's native-Win32-threads model, under
 * which <gc.h> does NOT declare the `GC_pthread_*` wrappers (gc_config_macros.h:
 * GC_PTHREADS is gated on `!GC_WIN32_THREADS || GC_WIN32_PTHREADS`). The runtime
 * creates coroutine/worker threads with `GC_pthread_create` and links winpthreads
 * (`-lpthread`), so request bdwgc's pthreads-on-Win32 model — this declares the
 * wrappers and makes the collector register each winpthreads thread for stack
 * scanning. MSYS2's `mingw-w64-x86_64-gc` is built with POSIX (winpthreads)
 * thread support, so the wrappers also link. (I#198 / A4.) */
#define GC_WIN32_PTHREADS
#endif
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
#define GC_pthread_detach pthread_detach
#define GC_call_with_alloc_lock(fn, arg) ((fn)(arg))
struct GC_stack_base {
    void *mem_base;
};
static void *GC_get_my_stackbottom(struct GC_stack_base *sb) {
    sb->mem_base = NULL;
    return NULL;
}
#endif
#include <math.h>
#include <pthread.h>
#include <stdint.h>
/* ucontext (getcontext/makecontext/swapcontext) backs the M:N scheduler's
 * green-thread context switch (ticket #47). Validated on Linux x86_64 and
 * aarch64 (CI, ticket #204); see the scheduler block below for the arch note.
 * POSIX-only: Windows has no ucontext — the A1 context-switch seam
 * (zap_ctx_swap/zap_ctx_make, defined just before the scheduler section) wraps
 * it here and is replaced by a hand-written asm backend on Windows (I#195). */
#if defined(ZAP_PLATFORM_POSIX)
#include <ucontext.h>
#endif
#include <stdio.h>
#include <stdarg.h> /* va_list for the variadic zap_select (D-0031) */
#include <stdlib.h>
#include <string.h>
#include <time.h>   /* clock_gettime for the scheduler idle wait (#47) */
#include <unistd.h> /* sysconf(_SC_NPROCESSORS_ONLN) for the M count (#47) */
#include <signal.h> /* SIGURG / sigaction / pthread_kill: async preemption (#172) */
#include <setjmp.h> /* zap_repl_call: catch an uncaught exception at a REPL entry (I#159) */
#if defined(__linux__)
#include <sys/random.h> /* getentropy for the per-process hash seed (#1064) */
#endif

#if defined(ZAP_PLATFORM_WINDOWS)
/* Windows system headers (platform epic #158 / Area A4, I#198). The POSIX
 * subsystems the runtime is built on — process/fs/env (#97), the TCP sockets the
 * netpoller drives (#104), the CSPRNG (#105), and the A3 signal/preempt/fault
 * primitives (#197) — reach Win32 here, behind the `ZAP_PLATFORM_WINDOWS` seam,
 * keeping the POSIX branches byte-for-byte unchanged. `<winsock2.h>` MUST precede
 * `<windows.h>` (otherwise the legacy `<winsock.h>` it pulls in clashes), and
 * WIN32_LEAN_AND_MEAN / NOMINMAX keep windows.h from defining `min`/`max` and the
 * GDI surface that would collide with runtime identifiers. winpthreads provides
 * `<pthread.h>` and a native-HANDLE accessor used by the preempt monitor. */
#define WIN32_LEAN_AND_MEAN 1
#define NOMINMAX 1
#define NOGDI 1
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <bcrypt.h>
#include <direct.h> /* _getcwd / _chdir / _mkdir / _rmdir                        */
#include <io.h>     /* _open / _read / _write / _close / _lseeki64 / _access     */
#include <process.h> /* _getpid                                                  */
#include <sys/stat.h>
#include <sys/types.h>
/* A3 (I#197) Windows primitives, implemented in the self-contained zap_win_a3.c
 * and wired into the scheduler/#77 signal/fault sections below (I#198). Declared
 * here, before the first use (the preempt monitor, the signal section, and the
 * per-worker fault headroom in worker_main). */
extern void (*zap_win_preempt_target)(void);
extern int zap_win_preempt_try(void *hthread, uintptr_t lo, uintptr_t hi,
                               volatile int *no_preempt, volatile LONG *in_hijack);
extern void (*zap_win_fault_describe)(char *buf, size_t n);
extern void zap_win_install_fault_filter(void);
extern void zap_win_fault_thread_init(void);
extern int (*zap_win_console_deliver)(int signo);
extern int zap_win_console_install(void);
extern void zap_win_console_uninstall(void);

/* The Zap `Signal` prelude enum uses the Linux signal numbers as its
 * discriminants (crates/syntax/src/parser.rs), and codegen emits those values;
 * the #77 dispatch tables are indexed by them. MinGW-w64's <signal.h> defines a
 * small, differently-numbered subset (SIGINT/SIGILL/SIGFPE/SIGSEGV/SIGTERM happen
 * to match Linux). Define the remaining names to their LINUX values so the signal
 * section compiles and the table indexing stays consistent with codegen; the
 * Windows-reachable subset is only SIGINT/SIGQUIT/SIGTERM (console events, see
 * zap_win_a3.c), and every other discriminant is inert on Windows. */
#ifndef SIGHUP
#define SIGHUP 1
#endif
#ifndef SIGQUIT
#define SIGQUIT 3
#endif
#ifndef SIGBUS
#define SIGBUS 7
#endif
#ifndef SIGKILL
#define SIGKILL 9
#endif
#ifndef SIGUSR1
#define SIGUSR1 10
#endif
#ifndef SIGUSR2
#define SIGUSR2 12
#endif
#ifndef SIGPIPE
#define SIGPIPE 13
#endif
#ifndef SIGALRM
#define SIGALRM 14
#endif
#ifndef SIGCHLD
#define SIGCHLD 17
#endif
#ifndef SIGCONT
#define SIGCONT 18
#endif
#ifndef SIGSTOP
#define SIGSTOP 19
#endif
#ifndef SIGTSTP
#define SIGTSTP 20
#endif
#ifndef SIGURG
#define SIGURG 23
#endif
#ifndef SIGPROF
#define SIGPROF 27
#endif
#ifndef SIGWINCH
#define SIGWINCH 28
#endif
#endif

/* Portable setenv (I#198): POSIX has setenv(name, value, overwrite); the MSVCRT
 * MinGW-w64 links against has no setenv, so route Windows through _putenv_s with
 * the same overwrite semantics. Returns 0 on success, non-zero on failure. Used
 * by the scheduler's MAXPROCS seed and os.setenv. */
static int zap_setenv_compat(const char *name, const char *value, int overwrite) {
#if defined(ZAP_PLATFORM_WINDOWS)
    if (!overwrite && getenv(name) != NULL) {
        return 0;
    }
    return _putenv_s(name, value);
#else
    return setenv(name, value, overwrite);
#endif
}

#if defined(ZAP_PLATFORM_WINDOWS)
/* POSIX reentrant time helpers the datetime section (#99) uses. MinGW-w64 ships
 * the MSVCRT *_s variants (note the reversed argument order) and _mkgmtime for
 * the timegm glibc extension; alias them so the shared civil-time code compiles
 * unchanged. */
static struct tm *zap_win_localtime_r(const time_t *t, struct tm *out) {
    return localtime_s(out, t) == 0 ? out : NULL;
}
static struct tm *zap_win_gmtime_r(const time_t *t, struct tm *out) {
    return gmtime_s(out, t) == 0 ? out : NULL;
}
#define localtime_r zap_win_localtime_r
#define gmtime_r zap_win_gmtime_r
#define timegm _mkgmtime
#endif

/* PCRE2, 8-bit code units (ticket #23, D-0010). Must be defined before the
 * header so the `pcre2_*` names resolve to the 8-bit API. Linked with
 * `-lpcre2-8` (see build_executable). Gated OFF with `-DZAP_NO_PCRE2` (#853)
 * when the program never reaches the regex seam: the `zap_regex_*` entry points
 * below become catchable-error stubs and no libpcre2 is linked, so this header
 * must not be required in that build. */
#if !defined(ZAP_NO_PCRE2)
#define PCRE2_CODE_UNIT_WIDTH 8
#include <pcre2.h>
#endif

/* System zlib (ticket #106, D-0060): DEFLATE with the zlib (RFC 1950, Adler-32)
 * and gzip (RFC 1952, ISO CRC32) wrappers, plus the crc32/adler32 checksums.
 * Linked with `-lz` (see build_executable); the dev container / CI provide
 * `zlib1g-dev` and it ships on macOS. The zlib/gzip section lives at the bottom
 * of this file. Gated OFF with `-DZAP_NO_ZLIB` (#853) when the program never
 * reaches the zlib/gzip seam: those entry points become catchable-error stubs
 * and no libz is linked. */
#if !defined(ZAP_NO_ZLIB)
#include <zlib.h>
#endif

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
    ZAP_TD_SCOPE = 10, /* Scope: excs buffer of ZapStr* pending failures        */
    ZAP_TD_ATOMICARRAY = 11, /* ZapAtomicArray: slots buffer of CAS-able words  */
    ZAP_TD_DEQUE = 12  /* ZapDeque: ring buffer of element words (#983)         */
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
extern const ZapTypeDesc zap_desc_atomicarray; /* AtomicArray[T] (#844)           */

/* Preemption guard (ticket #172): mark a window in which the current coroutine
 * must not be async-preempted (it is inside a runtime critical section that
 * holds a lock or the allocator). Defined in the scheduler section below; only
 * the Boehm signal-based path consults it, but the paired calls compile on both
 * builds (a harmless no-op when no coroutine is running). */
static void zap_np_enter(void);
static void zap_np_leave(void);
/* The current coroutine's no-preempt counter, or NULL off a coroutine (defined in
 * the scheduler section). A `noinline` call that reads the thread-local FRESH, so
 * a caller may hold the returned pointer across code that cannot park (a libgc
 * call) and use it for both ends of a no-preempt window with ONE thread-local read
 * (I#891: cheaper than two, and immune to a stale thread pointer). */
static __attribute__((noinline)) int *zap_np_slot(void);
static inline void zap_np_begin(int *np) {
    if (np) {
        ++*np;
    }
}
static inline void zap_np_end(int *np) {
    if (np && *np > 0) {
        --*np;
    }
}

/* CPU profiler (ticket #78, D-0058). Defined in the profiler section at the end
 * of this file (it needs the scheduler globals and, under gc-immix, the mutator
 * state from the #included collector). Called earlier here: the SIGUSR2 default
 * handler toggles it, `zap_sched_init` honours the `ZAP_PROFILE` env once the
 * worker Ms exist, and `zap_rt_join_all` stops/flushes and frees it at exit. */
void zap_profile_toggle(void);                /* SIGUSR2 default + zap inspect   */
void zap_profile_start(void);                 /* runtime.profile_start()         */
void zap_profile_stop(void);                  /* runtime.profile_stop()          */
static void zap_profile_env_init(void);       /* parse ZAP_PROFILE* at main top  */
static void zap_profile_env_maybe_start(void);/* auto-start once g_ms exists      */
static void zap_profile_teardown(void);       /* final free (all workers joined) */

/* Out-of-memory policy (D-0119, #697). A NULL result from the allocator means
 * heap exhaustion, and generated code stores into an allocation result WITHOUT a
 * null check — so an unchecked NULL becomes a wild nil-deref SIGSEGV at address 0
 * instead of a diagnosable error. This one collector-agnostic helper is the
 * single clean exit for every exhaustion path: it prints `zap: out of memory`
 * (an optional `detail` names the failing path) and aborts. `abort()` is the
 * established runtime-fatal convention in this file (see the list-sort, bytes and
 * container paths); it raises SIGABRT for a nonzero exit and leaves a core for
 * triage. OOM is process-fatal, not a catchable Zap exception: the mutator holds
 * unrooted temporaries at the raising allocation site, so unwinding through
 * zap_panic would be unsafe. Under Boehm the wrappers below call it when
 * GC_MALLOC / GC_MALLOC_ATOMIC return NULL (bdwgc's default GC_oom_fn returns 0);
 * under Immix zapgc.inc.c routes its map failures and the ZAP_GC_HEAP_LIMIT cap
 * through it so both collectors fail identically. */
__attribute__((noreturn)) static void zap_oom(const char *detail) {
    if (detail && *detail) {
        fprintf(stderr, "zap: out of memory (%s)\n", detail);
    } else {
        fprintf(stderr, "zap: out of memory\n");
    }
    abort();
}

/* Scope-bound arena / region allocator (#553, D-0158). Backend-uniform public
 * entry points: the precise-collector implementation is in zapgc.inc.c (pooled
 * non-moving regions, deterministic bulk free); the Boehm implementation is below
 * (cap-accounting over GC allocations). `arena` is an opaque control-block
 * pointer; a NULL alloc result signals cap / heap OOM so codegen raises
 * MemoryError. */
void *zap_arena_open(int64_t size_hint, int64_t cap);
void *zap_arena_alloc_bytes(void *arena, int64_t n);
void *zap_arena_alloc_atomic(void *arena, int64_t size);
void zap_arena_close(void *arena);
/* Raised (from a generated NULL-check on an arena allocation) when the arena's
 * `cap` is exceeded or the heap is exhausted. A catchable RuntimeError whose
 * message names it a memory error; noreturn. (A first-class MemoryError type is a
 * tracked follow-up — see the #553 epic.) */
void zap_arena_oom(void);

#ifdef ZAP_GC_IMMIX
/* Precise collector entry points (defined in zapgc.inc.c, #included below). */
void *zap_gc_alloc(const ZapTypeDesc *desc);
void *zap_gc_alloc_atomic(int64_t size);
void *zap_gc_alloc_var(const ZapTypeDesc *desc, int64_t size);
void zap_safepoint(void);
void zap_gc_add_root(void **slot);
void zap_gc_add_root_fn(void (*fn)(void));
void zap_gc_write_barrier(void **slot);
void zap_gc_mark(void *payload);
void zap_gc_collect(void);
/* Defined in zapgc.inc.c (#included below) but called earlier in this file
 * (the ticket-#77 SIGUSR1 stats handler), so it needs a forward prototype under
 * gc-immix to avoid an implicit-declaration / conflicting-types warning. */
void zap_gc_stats(int64_t *heap_bytes, int64_t *live_bytes);
/* Finalizers (ticket #92): register cleanup for an object, run once when it
 * becomes unreachable. `zap_py_wrap` uses it for the CPython-ref DECREF. */
void zap_gc_register_finalizer(void *payload, void (*fn)(void *obj, void *cd), void *cd);
int64_t zap_gc_finalizer_selftest(void); /* internal __gc_finalizer_selftest hook */
/* The per-thread shadow-stack head lives in `zap_shadow_head_tls` (zapgc.inc.c) and
 * is reached ONLY through the `noinline` accessor, never by a cached thread-local
 * address, so a coroutine that migrates OS threads at a safepoint cannot touch the
 * thread it left (I#891; generated code already did, I#159/#568). */
void **zap_shadow_head_addr(void);
#define zap_shadow_head (*zap_shadow_head_addr())
static void zap_enter_blocking(void); /* fwd (defined in zapgc.inc.c)           */
static void zap_leave_blocking(void);
static void zap_mutator_register(void);
static void zap_mutator_unregister(void); /* dedicated-M exit (#172)            */

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
#define zap_mutator_unregister() ((void)0)
/* Module-level global roots (D-0112, #658 Phase 1): a REAL exported no-op under
 * Boehm, not a macro, because compiler-generated code (`@__zap_init_globals`)
 * emits a `call zap_gc_add_root` for each pointer-typed module global and that
 * symbol must resolve at link time. The conservative collector already scans the
 * static data/BSS segment, so a module global is a root with no registration —
 * hence the empty body. (Under precise Immix the real implementation in
 * zapgc.inc.c records the slot so the collector marks and relocates through it.) */
void zap_gc_add_root(void **slot) { (void)slot; }
#define zap_gc_add_root_fn(fn) ((void)(fn))
/* Module-global write barrier (D-0112, #663): reassigning a mutable module-level
 * pointer global overwrites a permanent root slot. A REAL exported no-op under
 * Boehm, not a macro, because codegen emits a `call zap_gc_write_barrier` at each
 * reassignment site and the symbol must resolve at link time. The conservative
 * collector is non-moving and finds roots by scanning the data segment, so it
 * needs no remembered set — hence the empty body. (Under precise Immix the real
 * implementation in zapgc.inc.c is the record point for a future generational /
 * incremental cycle.) */
void zap_gc_write_barrier(void **slot) { (void)slot; }
#define zap_gc_mark(p) ((void)(p))
/* ZAP_GC_GUARDED_LAYER_BEGIN (Boehm) --------------------------------------------
 * EVERY call into libgc that takes (or may take) the collector's allocator lock
 * lives in this block, bracketed by a no-preempt window, and nothing outside the
 * `ZAP_GC_GUARDED_LAYER_*` blocks may name the raw entry points (a unit test in
 * zapc_codegen enforces it — I#888).
 *
 * Why: the allocator lock is a plain, non-recursive `pthread_mutex_t`. The Boehm
 * async-preemption handler (SIGURG, ticket #172) switches a coroutine away at an
 * arbitrary instruction unless it is inside a no-preempt window (`no_preempt > 0`).
 * A coroutine preempted while inside libgc holds the lock across the switch; the
 * coroutine run next on the SAME OS thread then blocks in its first allocation on
 * a lock its own thread already owns — a self-deadlock that wedges every worker.
 * The allocation helpers were always guarded; `gc.collect()` (`GC_gcollect`, which
 * holds the lock for the whole collection), the finalizer registration, the heap
 * statistics, the raw `zap_alloc`, the scheduler-infrastructure allocations and the
 * thread create/join/detach calls (each takes the lock) were not (I#888: a
 * coroutine preempted just inside `GC_gcollect` hung `gc_switch_alloc_stress`
 * ~7% of runs). */
static inline void *zap_gc_malloc(size_t n) {
    int *np = zap_np_slot();
    zap_np_begin(np);
    void *p = GC_MALLOC(n);
    zap_np_end(np);
    return p;
}
static inline void *zap_gc_malloc_atomic(size_t n) {
    int *np = zap_np_slot();
    zap_np_begin(np);
    void *p = GC_MALLOC_ATOMIC(n);
    zap_np_end(np);
    return p;
}
static inline void zap_gc_collect_guarded(void) {
    int *np = zap_np_slot();
    zap_np_begin(np);
    GC_gcollect();
    zap_np_end(np);
}
static inline void zap_gc_heap_sizes(size_t *heap, size_t *freeb) {
    int *np = zap_np_slot();
    zap_np_begin(np);
    *heap = GC_get_heap_size();
    *freeb = GC_get_free_bytes();
    zap_np_end(np);
}
/* Finalizers (ticket #92): forward to Boehm's registration so the one call site
 * (`zap_py_wrap`) is backend-uniform. Boehm's default finalizer already provides
 * exactly these semantics (one-shot, run when unreachable, outside the GC). */
static inline void zap_gc_register_finalizer(void *payload,
                                             void (*fn)(void *obj, void *cd), void *cd) {
    int *np = zap_np_slot();
    zap_np_begin(np);
    GC_register_finalizer(payload, (GC_finalization_proc)fn, cd, NULL, NULL);
    zap_np_end(np);
}
/* ZAP_GC_GUARDED_LAYER_END */

/* Every Boehm allocation checks the result: on heap exhaustion GC_MALLOC /
 * GC_MALLOC_ATOMIC return NULL (bdwgc's default GC_oom_fn returns 0), and codegen
 * stores into the returned pointer unchecked (#697), so an unchecked NULL faults
 * as a nil-deref SIGSEGV at address 0. Routing NULL through zap_oom turns that
 * wild fault into a clean `zap: out of memory` abort. */
static inline void *zap_alloc_desc(const ZapTypeDesc *d) {
    void *p = zap_gc_malloc((size_t)d->size);
    if (!p) zap_oom(NULL);
    return p;
}
static inline void *zap_obj(const ZapTypeDesc *d) {
    void *p = zap_gc_malloc((size_t)d->size);
    if (!p) zap_oom(NULL);
    return p;
}
static inline void *zap_buf(int64_t n) {
    void *p = zap_gc_malloc_atomic((size_t)n);
    if (!p) zap_oom(NULL);
    return p;
}
static inline void *zap_ptrbuf(int64_t n) {
    void *p = zap_gc_malloc((size_t)n);
    if (!p) zap_oom(NULL);
    return p;
}
#define ZAP_INFRA_ALLOC(n) zap_gc_malloc(n)
#endif

/* ZAP_GC_GUARDED_LAYER_BEGIN (threads) -------------------------------------------
 * Thread create / detach / join. Under Boehm these are `GC_pthread_*` and take the
 * allocator lock (thread-table update + allocation), and a coroutine can reach them
 * (`dedicated_pin`, the scope timers, the lazily-started pollers) — so they get the
 * same no-preempt window as every other libgc entry point (I#888). Under Immix they
 * are plain pthread calls and the window is a harmless no-op bracket. */
static inline int zap_gc_pthread_create(pthread_t *t, const pthread_attr_t *a,
                                        void *(*fn)(void *), void *arg) {
    int *np = zap_np_slot();
    zap_np_begin(np);
    int rc = GC_pthread_create(t, a, fn, arg);
    zap_np_end(np);
    return rc;
}
static inline int zap_gc_pthread_detach(pthread_t t) {
    int *np = zap_np_slot();
    zap_np_begin(np);
    int rc = GC_pthread_detach(t);
    zap_np_end(np);
    return rc;
}
static inline int zap_gc_pthread_join(pthread_t t, void **rv) {
    int *np = zap_np_slot();
    zap_np_begin(np);
    int rc = GC_pthread_join(t, rv);
    zap_np_end(np);
    return rc;
}
/* ZAP_GC_GUARDED_LAYER_END */

/* One-object string layout (#837, D-0145). The byte payload is a flexible array
 * member CONTIGUOUS with the header, so a string is a SINGLE GC object (header +
 * bytes), not a header plus a separate char buffer — halving both the allocation
 * count and the live-set tracing cost for string-heavy workloads (e.g. a dict of
 * 200k string keys). The bytes are NOT NUL-terminated; `len` is authoritative.
 *
 * `data` is a flexible array, so `s->data` still reads as a `char *` at every one
 * of the ~300 access sites, and because that address is recomputed from `s` on
 * each access it stays correct after a moving-GC relocation. The string
 * descriptor is therefore ATOMIC (no traced pointer field): the inline bytes are
 * never scanned as pointers, and evacuation copies the whole object by its stored
 * object size. Strings remain immutable and always own their bytes (slice/concat
 * copy), so one contiguous buffer never aliases another object. */
typedef struct {
    int64_t len;
    char data[];
} ZapStr;

/* Allocate a single-object string with room for `len` payload bytes (#837). The
 * caller sets the bytes via `s->data`; `->len` is pre-set. Backend-uniform: the
 * precise collector uses the variable-size entry with the (atomic) string
 * descriptor, so the object's stored size spans the inline bytes; Boehm uses an
 * atomic (pointer-free) block of the same total size. Allocation never collects
 * mid-call (it only flags a pending GC), so the returned raw pointer is stable
 * for the remainder of the calling runtime function. */
extern const ZapTypeDesc zap_desc_strobj;
static ZapStr *zap_str_alloc(int64_t len) {
    if (len < 0) {
        len = 0;
    }
    int64_t total = (int64_t)sizeof(ZapStr) + len;
#ifdef ZAP_GC_IMMIX
    ZapStr *s = (ZapStr *)zap_gc_alloc_var(&zap_desc_strobj, total);
#else
    ZapStr *s = (ZapStr *)zap_gc_malloc_atomic((size_t)total);
    if (!s) {
        zap_oom(NULL);
    }
#endif
    s->len = len;
    return s;
}

#ifndef ZAP_GC_IMMIX
/* Boehm arena (#553, D-0158). The conservative collector is non-moving, so arena
 * objects are "pinned" automatically and there is no region pool to draw from:
 * the arena is a cap-accounting control block over ordinary GC allocations. Leaf-
 * only means the objects hold no pointers; they are reclaimed by the collector
 * when unreachable (the deterministic bulk-free is an Immix-only guarantee). `cap`
 * still bounds total reservation, returning NULL (-> MemoryError) past it. */
typedef struct ZapArena {
    size_t reserved;
    size_t cap;
} ZapArena;

void *zap_arena_open(int64_t size_hint, int64_t cap) {
    (void)size_hint; /* no pooled reservation under Boehm */
    ZapArena *a = (ZapArena *)malloc(sizeof(ZapArena));
    if (!a) {
        zap_oom("arena open");
    }
    a->reserved = 0;
    a->cap = cap > 0 ? (size_t)cap : 0;
    /* A runtime `size > cap` (both set) can never be satisfied (#553, D-0158):
     * fail the open so the generated NULL-check raises. The constant case is a
     * compile error (ZAP5132). */
    if (cap > 0 && size_hint > 0 && size_hint > cap) {
        free(a);
        return NULL;
    }
    return a;
}

static void *zap_arena_boehm_alloc(ZapArena *a, size_t n, int atomic) {
    if (a->cap && a->reserved + n > a->cap) {
        return NULL; /* cap exceeded -> MemoryError */
    }
    void *p = atomic ? zap_gc_malloc_atomic(n) : zap_gc_malloc(n);
    if (!p) {
        return NULL; /* heap OOM -> MemoryError */
    }
    a->reserved += n;
    return p;
}

void *zap_arena_alloc_bytes(void *arena, int64_t n) {
    ZapArena *a = (ZapArena *)arena;
    if (n < 0) {
        n = 0;
    }
    size_t total = sizeof(ZapStr) + (size_t)n;
    ZapStr *s = (ZapStr *)zap_arena_boehm_alloc(a, total, 1 /* atomic/leaf */);
    if (s) {
        s->len = n; /* GC_MALLOC_ATOMIC returns zeroed memory */
    }
    return s;
}

void *zap_arena_alloc_atomic(void *arena, int64_t size) {
    ZapArena *a = (ZapArena *)arena;
    return zap_arena_boehm_alloc(a, (size_t)(size < 0 ? 0 : size), 1 /* atomic/leaf */);
}

void zap_arena_close(void *arena) {
    /* Objects are GC-managed; just drop the cap-accounting control block. */
    free(arena);
}
#endif /* !ZAP_GC_IMMIX */

/* A stack-allocated, non-GC string with the one-object layout (#837), for a short
 * COMPILE-TIME-sized borrowed C string passed to a `ZapStr *`-consuming call
 * without touching the GC heap. Kept off the heap so no moving-GC relocation can
 * happen while a transport routine holds the pointer across a blocking park
 * (where a heap string's C pointer would be unrooted). `ident` becomes a
 * `ZapStr *`; `n` MUST be a compile-time constant (fixed-size inline buffer,
 * 8-byte aligned via the leading int64 length). */
#define ZAP_STACK_STR(ident, cstr, n)                                          \
    struct {                                                                   \
        int64_t len;                                                           \
        char data[(n) > 0 ? (n) : 1];                                          \
    } ident##_buf_;                                                            \
    ident##_buf_.len = (int64_t)(n);                                           \
    if ((n) > 0)                                                               \
        memcpy(ident##_buf_.data, (cstr), (size_t)(n));                        \
    ZapStr *ident = (ZapStr *)&ident##_buf_

/* Typed Zap exception values (I#471). The thrown C++ object still carries one
 * pointer, but that pointer now identifies the Zap type and its message.
 * `cause` is the explicit `raise ... from <cause>` chain (#604, D-0103): NULL
 * for an ordinary raise, else the causing exception object. It is traced by the
 * precise descriptor so the chain stays live until the exception is reported. */
typedef struct {
    uint64_t magic;
    int64_t type_id;
    void *message;
    void *cause;
    /* `children` carries the real sub-exception objects of an `ExceptionGroup`
     * (#605, D-0129): a `ZapList` of exception-object pointers, or NULL for a
     * non-group exception. It is what `except* E as eg` splits by type and what
     * `eg.exceptions` iterates. Traced precisely (the list in turn traces its
     * elements), so the whole group stays live while it is in flight. */
    void *children;
} ZapException;
#define ZAP_EXCEPTION_MAGIC UINT64_C(0x5a41504558434550)
static const int32_t zap_exc_ptr_offsets[] = {
    (int32_t)offsetof(ZapException, message),
    (int32_t)offsetof(ZapException, cause),
    (int32_t)offsetof(ZapException, children),
};
static const ZapTypeDesc zap_desc_exception = {
    (int64_t)sizeof(ZapException), ZAP_TD_PRECISE, 3, zap_exc_ptr_offsets};
static void *zap_current_exc;

enum {
    ZAP_EXC_BASE = 1,
    ZAP_EXC_EXCEPTION = 2,
    ZAP_EXC_VALUE_ERROR = 3,
    ZAP_EXC_TYPE_ERROR = 4,
    ZAP_EXC_LOOKUP_ERROR = 5,
    ZAP_EXC_INDEX_ERROR = 6,
    ZAP_EXC_KEY_ERROR = 7,
    ZAP_EXC_ARITHMETIC_ERROR = 8,
    ZAP_EXC_ZERO_DIVISION_ERROR = 9,
    ZAP_EXC_RUNTIME_ERROR = 10,
    ZAP_EXC_STOP_ITERATION = 11,
    ZAP_EXC_CANCELLED = 12,
    ZAP_EXC_EXCEPTION_GROUP = 13,
    /* Raised *to the caller* when a `scope(timeout=...)` deadline expires (I#475).
     * The caller is outside the scope and is not itself cancelled, so this is an
     * ordinary catchable exception under `Exception` — distinct from `Cancelled`,
     * which the cancelled children receive and which stays outside `Exception`. */
    ZAP_EXC_SCOPE_TIMEOUT = 14,
    /* Raised by a socket read that exceeds its `net.set_read_timeout` deadline
     * (I#485). Distinct from a clean EOF (an empty `recv` result) so a server can
     * tell an idle/read timeout apart from a peer close. An ordinary catchable
     * exception under `Exception`. */
    ZAP_EXC_TIMEOUT_ERROR = 15,
    /* Raised by a failed `assert` (#374/B7, D-0054). An ordinary catchable
     * exception under `Exception`, so `except AssertionError` (and `except
     * Exception`) catches it; the power-assert diagnostic is its message. */
    ZAP_EXC_ASSERTION_ERROR = 16,
    /* Raised by `input()` at end-of-input (#1034->#1036, D-0215): a clean
     * Ctrl-D / closed-stdin signal, matching CPython's `input()` raising
     * `EOFError`. An ordinary catchable exception under `Exception`. */
    ZAP_EXC_EOF_ERROR = 17,
};

void *zap_exception_new(int64_t type_id, void *message) {
    ZapException *e = (ZapException *)zap_obj(&zap_desc_exception);
    e->magic = ZAP_EXCEPTION_MAGIC;
    e->type_id = type_id;
    e->message = message;
    e->cause = NULL;
    e->children = NULL;
    return e;
}

void *zap_exception_message(void *exc) {
    ZapException *e = (ZapException *)exc;
    return e && e->magic == ZAP_EXCEPTION_MAGIC ? e->message : NULL;
}

/* Record the explicit `raise ... from <cause>` chain (#604, D-0103). `cause`
 * is another exception object; a null/untyped exc or cause is a no-op. */
void zap_exception_set_cause(void *exc, void *cause) {
    ZapException *e = (ZapException *)exc;
    if (e && e->magic == ZAP_EXCEPTION_MAGIC) {
        e->cause = cause;
    }
}

/* The causing exception object set by `raise ... from`, or NULL. */
void *zap_exception_cause(void *exc) {
    ZapException *e = (ZapException *)exc;
    return e && e->magic == ZAP_EXCEPTION_MAGIC ? e->cause : NULL;
}

void *zap_str_new(const char *data, int64_t len); /* defined below */

/* The cause's message string, surfaced to Zap as `<caught>.cause` (#604).
 * An empty string when there is no explicit cause, so Zap code can read and
 * print it without a None check. */
void *zap_exception_cause_message(void *exc) {
    void *cause = zap_exception_cause(exc);
    void *msg = cause ? zap_exception_message(cause) : NULL;
    return msg ? msg : zap_str_new("", 0);
}

static int64_t zap_exception_parent(int64_t type_id) {
    switch (type_id) {
    case ZAP_EXC_EXCEPTION: return ZAP_EXC_BASE;
    case ZAP_EXC_VALUE_ERROR: case ZAP_EXC_TYPE_ERROR: case ZAP_EXC_LOOKUP_ERROR:
    case ZAP_EXC_ARITHMETIC_ERROR: case ZAP_EXC_RUNTIME_ERROR:
    case ZAP_EXC_STOP_ITERATION: case ZAP_EXC_EXCEPTION_GROUP:
    case ZAP_EXC_SCOPE_TIMEOUT: case ZAP_EXC_TIMEOUT_ERROR:
    case ZAP_EXC_ASSERTION_ERROR: case ZAP_EXC_EOF_ERROR:
        return ZAP_EXC_EXCEPTION;
    case ZAP_EXC_INDEX_ERROR: case ZAP_EXC_KEY_ERROR: return ZAP_EXC_LOOKUP_ERROR;
    case ZAP_EXC_ZERO_DIVISION_ERROR: return ZAP_EXC_ARITHMETIC_ERROR;
    case ZAP_EXC_CANCELLED: return ZAP_EXC_BASE;
    default: return type_id >= 1024 ? ZAP_EXC_EXCEPTION : 0;
    }
}

int64_t zap_exception_is_a(void *exc, int64_t expected) {
    if (!exc) return 0;
    ZapException *e = (ZapException *)exc;
    if (e->magic != ZAP_EXCEPTION_MAGIC) return 0;
    int64_t actual = e->type_id;
    while (actual != 0) {
        if (actual == expected) return 1;
        actual = zap_exception_parent(actual);
    }
    return 0;
}

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
    void *p = zap_gc_malloc((size_t)size); /* guarded: I#888 */
    if (!p) zap_oom(NULL); /* unchecked NULL would nil-deref in generated code (#697) */
    return p;
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
void zap_gc_collect(void) { zap_gc_collect_guarded(); } /* I#888: no-preempt window */
void zap_gc_stats(int64_t *heap_bytes, int64_t *live_bytes) {
    size_t heap, freeb;
    zap_gc_heap_sizes(&heap, &freeb);
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
    if (len < 0) {
        len = 0;
    }
    ZapStr *s = zap_str_alloc(len);
    if (len > 0) {
        memcpy(s->data, data, (size_t)len);
    }
    return s;
}

/* Complex scalars are immutable, pointer-free GC objects. Keeping their two
 * doubles behind a stable pointer makes the ABI uniform for calls, locals and
 * return values on every native target. */
typedef struct {
    double re;
    double im;
} ZapComplex;
void zap_panic(void *exc);
void zap_raise(void *exc);
void zap_panic_typed(int64_t type_id, void *message);

static ZapComplex *zap_complex_alloc(double re, double im) {
    ZapComplex *z = (ZapComplex *)zap_buf((int64_t)sizeof(ZapComplex));
    z->re = re;
    z->im = im;
    return z;
}

void *zap_complex_new(double re, double im) { return zap_complex_alloc(re, im); }
void *zap_complex_from_f64(double re) { return zap_complex_alloc(re, 0.0); }
void *zap_complex_add(void *ap, void *bp) {
    const ZapComplex *a = (const ZapComplex *)ap;
    const ZapComplex *b = (const ZapComplex *)bp;
    return zap_complex_alloc(a->re + b->re, a->im + b->im);
}
void *zap_complex_sub(void *ap, void *bp) {
    const ZapComplex *a = (const ZapComplex *)ap;
    const ZapComplex *b = (const ZapComplex *)bp;
    return zap_complex_alloc(a->re - b->re, a->im - b->im);
}
void *zap_complex_mul(void *ap, void *bp) {
    const ZapComplex *a = (const ZapComplex *)ap;
    const ZapComplex *b = (const ZapComplex *)bp;
    return zap_complex_alloc(a->re * b->re - a->im * b->im,
                             a->re * b->im + a->im * b->re);
}
void *zap_complex_div(void *ap, void *bp) {
    const ZapComplex *a = (const ZapComplex *)ap;
    const ZapComplex *b = (const ZapComplex *)bp;
    double d = b->re * b->re + b->im * b->im;
    if (d == 0.0) {
        zap_panic_typed(ZAP_EXC_ZERO_DIVISION_ERROR,
                        zap_str_new("complex division by zero", 24));
    }
    return zap_complex_alloc((a->re * b->re + a->im * b->im) / d,
                             (a->im * b->re - a->re * b->im) / d);
}
void *zap_complex_pow(void *ap, void *bp) {
    const ZapComplex *a = (const ZapComplex *)ap;
    const ZapComplex *b = (const ZapComplex *)bp;
    double radius = hypot(a->re, a->im);
    if (radius == 0.0) {
        if (b->re == 0.0 && b->im == 0.0) {
            return zap_complex_alloc(1.0, 0.0);
        }
        return zap_complex_alloc(0.0, 0.0);
    }
    double log_r = log(radius);
    double theta = atan2(a->im, a->re);
    double magnitude = exp(b->re * log_r - b->im * theta);
    double phase = b->im * log_r + b->re * theta;
    return zap_complex_alloc(magnitude * cos(phase), magnitude * sin(phase));
}
int64_t zap_complex_eq(void *ap, void *bp) {
    const ZapComplex *a = (const ZapComplex *)ap;
    const ZapComplex *b = (const ZapComplex *)bp;
    return a->re == b->re && a->im == b->im;
}
int64_t zap_complex_truthy(void *p) {
    const ZapComplex *z = (const ZapComplex *)p;
    return z->re != 0.0 || z->im != 0.0;
}
void *zap_complex_neg(void *p) {
    const ZapComplex *z = (const ZapComplex *)p;
    return zap_complex_alloc(-z->re, -z->im);
}
void *zap_complex_to_str(void *p) {
    const ZapComplex *z = (const ZapComplex *)p;
    char buf[128];
    int n = snprintf(buf, sizeof(buf), "(%g%+gj)", z->re, z->im);
    return zap_str_new(buf, (int64_t)n);
}

/* Arbitrary precision integers use immutable base-2^32 limbs in pointer-free
 * GC objects. Temporary limb buffers use malloc and only the final result is
 * published to the GC, so intermediate C locals never need to be traced. */
typedef struct {
    int32_t sign;
    uint32_t n;
    uint32_t limb[];
} ZapBigInt;

typedef struct {
    uint32_t n;
    uint32_t *limb;
} ZapBigRaw;

static void zap_big_trim(ZapBigRaw *a) {
    while (a->n && a->limb[a->n - 1] == 0) a->n--;
}
static int zap_big_cmp_abs(const ZapBigInt *a, const ZapBigInt *b) {
    if (a->n != b->n) return a->n < b->n ? -1 : 1;
    for (uint32_t i = a->n; i; i--) {
        if (a->limb[i - 1] != b->limb[i - 1])
            return a->limb[i - 1] < b->limb[i - 1] ? -1 : 1;
    }
    return 0;
}
static ZapBigInt *zap_big_make(int sign, const uint32_t *limbs, uint32_t n) {
    while (n && limbs[n - 1] == 0) n--;
    ZapBigInt *v = (ZapBigInt *)zap_buf((int64_t)(sizeof(ZapBigInt) + n * sizeof(uint32_t)));
    v->sign = n ? (sign < 0 ? -1 : 1) : 0;
    v->n = n;
    if (n) memcpy(v->limb, limbs, n * sizeof(uint32_t));
    return v;
}
static ZapBigRaw zap_big_raw_copy(const ZapBigInt *a) {
    ZapBigRaw r = {a->n, NULL};
    if (r.n) {
        r.limb = (uint32_t *)calloc(r.n, sizeof(uint32_t));
        if (!r.limb) abort();
        memcpy(r.limb, a->limb, r.n * sizeof(uint32_t));
    }
    return r;
}
static ZapBigRaw zap_big_raw_mul(const uint32_t *a, uint32_t an,
                                const uint32_t *b, uint32_t bn) {
    ZapBigRaw r = {an + bn, NULL};
    r.limb = (uint32_t *)calloc(r.n ? r.n : 1, sizeof(uint32_t));
    if (!r.limb) abort();
    for (uint32_t i = 0; i < an; i++) {
        uint64_t carry = 0;
        for (uint32_t j = 0; j < bn; j++) {
            uint64_t x = (uint64_t)a[i] * b[j] + r.limb[i + j] + carry;
            r.limb[i + j] = (uint32_t)x;
            carry = x >> 32;
        }
        uint32_t k = i + bn;
        while (carry && k < r.n) {
            uint64_t x = (uint64_t)r.limb[k] + carry;
            r.limb[k++] = (uint32_t)x;
            carry = x >> 32;
        }
    }
    zap_big_trim(&r);
    return r;
}
static int zap_big_raw_cmp(const uint32_t *a, uint32_t an,
                           const uint32_t *b, uint32_t bn) {
    if (an != bn) return an < bn ? -1 : 1;
    for (uint32_t i = an; i; i--) {
        if (a[i - 1] != b[i - 1]) return a[i - 1] < b[i - 1] ? -1 : 1;
    }
    return 0;
}
static void zap_big_raw_sub(uint32_t *a, uint32_t *an,
                            const uint32_t *b, uint32_t bn) {
    uint64_t borrow = 0;
    for (uint32_t i = 0; i < *an; i++) {
        uint64_t av = a[i];
        uint64_t bv = (i < bn ? b[i] : 0) + borrow;
        a[i] = (uint32_t)(av - bv);
        borrow = av < bv;
    }
    while (*an && a[*an - 1] == 0) (*an)--;
}
static void zap_big_raw_add(uint32_t *a, uint32_t *an,
                            const uint32_t *b, uint32_t bn, uint32_t cap) {
    uint64_t carry = 0;
    uint32_t n = *an > bn ? *an : bn;
    for (uint32_t i = 0; i < n; i++) {
        uint64_t x = (i < *an ? a[i] : 0) + (uint64_t)(i < bn ? b[i] : 0) + carry;
        a[i] = (uint32_t)x;
        carry = x >> 32;
    }
    *an = n;
    if (carry && n < cap) a[(*an)++] = (uint32_t)carry;
}
/* Unsigned raw divmod on base-2^32 little-endian limb buffers. Computes
 * q = a / b and r = a % b (both non-negative). The inputs are trimmed
 * magnitudes (top limb non-zero, or length 0 for zero); the outputs are
 * trimmed and malloc-allocated so callers can free()/realloc() them.
 *
 * Historically this walked the dividend bit-by-bit, shifting every active
 * remainder limb on each of `an*32` steps (cost ~ numerator_bits *
 * remainder_limbs). A near-equal-length division with a one-limb quotient
 * therefore paid a full bit-length scan — the pidigits hot path (#1084).
 *
 * This is a word-wise long division: a single-limb fast path (schoolbook
 * division by a scalar) and, for multi-limb divisors, Knuth Algorithm D
 * (TAOCP vol. 2, 4.3.1): normalize so the divisor's top limb has its high
 * bit set, estimate each quotient limb from the top two numerator limbs
 * over the top divisor limb (with the two-limb correction), multiply-and-
 * subtract, add back on the rare over-estimate, then de-normalize the
 * remainder. A small quotient now costs ~O(n) limb operations. Sign and
 * floor/modulo semantics are applied by the signed wrapper and are
 * unchanged. */
static void zap_big_raw_divmod(const uint32_t *a, uint32_t an,
                               const uint32_t *b, uint32_t bn,
                               ZapBigRaw *q, ZapBigRaw *r) {
    /* Divide-by-zero: defensive — the signed wrapper already raises before
     * reaching here. Return 0/0 with freeable buffers. */
    if (!bn) {
        q->n = 0; q->limb = (uint32_t *)calloc(1, sizeof(uint32_t));
        r->n = 0; r->limb = (uint32_t *)calloc(1, sizeof(uint32_t));
        if (!q->limb || !r->limb) abort();
        return;
    }
    /* a < b (also covers a == 0): quotient 0, remainder = a. */
    if (zap_big_raw_cmp(a, an, b, bn) < 0) {
        q->n = 0; q->limb = (uint32_t *)calloc(1, sizeof(uint32_t));
        r->limb = (uint32_t *)calloc(an ? an : 1, sizeof(uint32_t));
        if (!q->limb || !r->limb) abort();
        if (an) memcpy(r->limb, a, an * sizeof(uint32_t));
        r->n = an; zap_big_trim(r);
        return;
    }
    /* Single-limb divisor: schoolbook long division by a scalar. */
    if (bn == 1) {
        uint32_t d = b[0];
        q->limb = (uint32_t *)calloc(an, sizeof(uint32_t));
        if (!q->limb) abort();
        q->n = an;
        uint64_t rem = 0;
        for (uint32_t i = an; i; i--) {
            uint64_t cur = (rem << 32) | a[i - 1];
            q->limb[i - 1] = (uint32_t)(cur / d);
            rem = cur % d;
        }
        zap_big_trim(q);
        r->limb = (uint32_t *)calloc(1, sizeof(uint32_t));
        if (!r->limb) abort();
        r->limb[0] = (uint32_t)rem;
        r->n = rem ? 1 : 0;
        return;
    }
    /* Knuth Algorithm D. Here an >= bn >= 2. */
    uint32_t m = an, n = bn;
    int s = 0;
    for (uint32_t top = b[n - 1]; !(top & 0x80000000u); top <<= 1) s++;
    uint32_t *vn = (uint32_t *)calloc(n, sizeof(uint32_t));
    uint32_t *un = (uint32_t *)calloc((size_t)m + 1, sizeof(uint32_t));
    if (!vn || !un) abort();
    /* Normalize the divisor so vn[n-1] has its high bit set. */
    for (uint32_t i = n - 1; i; i--)
        vn[i] = (b[i] << s) | (s ? (uint32_t)((uint64_t)b[i - 1] >> (32 - s)) : 0u);
    vn[0] = b[0] << s;
    /* Normalize the dividend by the same shift (gains one high limb). */
    un[m] = s ? (uint32_t)((uint64_t)a[m - 1] >> (32 - s)) : 0u;
    for (uint32_t i = m - 1; i; i--)
        un[i] = (a[i] << s) | (s ? (uint32_t)((uint64_t)a[i - 1] >> (32 - s)) : 0u);
    un[0] = a[0] << s;

    q->limb = (uint32_t *)calloc((size_t)(m - n) + 1, sizeof(uint32_t));
    if (!q->limb) abort();
    q->n = m - n + 1;

    const uint64_t BASE = 0x100000000ull;
    for (uint32_t j = m - n + 1; j--;) {
        /* Estimate quotient limb from the top two numerator limbs. */
        uint64_t num = (uint64_t)un[j + n] * BASE + un[j + n - 1];
        uint64_t qhat = num / vn[n - 1];
        uint64_t rhat = num % vn[n - 1];
        while (qhat >= BASE ||
               qhat * vn[n - 2] > rhat * BASE + un[j + n - 2]) {
            qhat--;
            rhat += vn[n - 1];
            if (rhat >= BASE) break;
        }
        /* Multiply divisor by qhat and subtract from the numerator window. */
        int64_t k = 0;
        for (uint32_t i = 0; i < n; i++) {
            uint64_t p = qhat * vn[i];
            int64_t t = (int64_t)un[j + i] - k - (int64_t)(uint32_t)p;
            un[j + i] = (uint32_t)t;
            k = (int64_t)(p >> 32) - (t >> 32);
        }
        int64_t t = (int64_t)un[j + n] - k;
        un[j + n] = (uint32_t)t;
        /* Over-estimate (by at most one): add the divisor back. */
        if (t < 0) {
            qhat--;
            uint64_t carry = 0;
            for (uint32_t i = 0; i < n; i++) {
                uint64_t sum = (uint64_t)un[j + i] + vn[i] + carry;
                un[j + i] = (uint32_t)sum;
                carry = sum >> 32;
            }
            un[j + n] = (uint32_t)(un[j + n] + carry);
        }
        q->limb[j] = (uint32_t)qhat;
    }
    zap_big_trim(q);
    /* De-normalize the remainder: r = (normalized remainder) >> s. */
    r->limb = (uint32_t *)calloc(n, sizeof(uint32_t));
    if (!r->limb) abort();
    for (uint32_t i = 0; i < n; i++)
        r->limb[i] = s ? (uint32_t)((un[i] >> s) | ((uint64_t)un[i + 1] << (32 - s)))
                       : un[i];
    r->n = n; zap_big_trim(r);
    free(un); free(vn);
}
static ZapBigInt *zap_big_parse_chars(const char *s, int64_t len) {
    int sign = 1;
    int64_t i = 0;
    if (i < len && (s[i] == '-' || s[i] == '+')) {
        if (s[i++] == '-') sign = -1;
    }
    unsigned radix = 10;
    if (i + 1 < len && s[i] == '0') {
        if (s[i + 1] == 'x' || s[i + 1] == 'X') radix = 16;
        else if (s[i + 1] == 'o' || s[i + 1] == 'O') radix = 8;
        else if (s[i + 1] == 'b' || s[i + 1] == 'B') radix = 2;
        if (radix != 10) i += 2;
    }
    uint32_t cap = (uint32_t)(len * 2 / 8 + 2);
    uint32_t *limbs = (uint32_t *)calloc(cap, sizeof(uint32_t));
    if (!limbs) abort();
    uint32_t n = 0;
    for (; i < len; i++) {
        unsigned char c = (unsigned char)s[i];
        if (c == '_') continue;
        unsigned d = c >= '0' && c <= '9' ? c - '0'
                   : c >= 'a' && c <= 'f' ? c - 'a' + 10
                   : c >= 'A' && c <= 'F' ? c - 'A' + 10 : radix;
        if (d >= radix) {
            free(limbs);
            zap_panic(zap_str_new("invalid bigint literal", 22));
            return NULL;
        }
        uint64_t carry = d;
        for (uint32_t j = 0; j < n; j++) {
            uint64_t x = (uint64_t)limbs[j] * radix + carry;
            limbs[j] = (uint32_t)x;
            carry = x >> 32;
        }
        if (carry && n < cap) limbs[n++] = (uint32_t)carry;
    }
    ZapBigInt *v = zap_big_make(sign, limbs, n);
    free(limbs);
    return v;
}
void *zap_bigint_from_str(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    return zap_big_parse_chars(s->data, s->len);
}
void *zap_bigint_from_i64(int64_t x) {
    uint64_t mag = x < 0 ? (uint64_t)(-(x + 1)) + 1 : (uint64_t)x;
    uint32_t limbs[2] = {(uint32_t)mag, (uint32_t)(mag >> 32)};
    return zap_big_make(x < 0 ? -1 : 1, limbs, limbs[1] ? 2 : limbs[0] ? 1 : 0);
}
void *zap_bigint_from_f64(double x) {
    if (!isfinite(x)) zap_panic(zap_str_new("cannot convert non-finite float to bigint", 41));
    char buf[384];
    int n = snprintf(buf, sizeof(buf), "%.0f", trunc(x));
    return zap_big_parse_chars(buf, n);
}
double zap_bigint_to_f64(void *p) {
    const ZapBigInt *a = (const ZapBigInt *)p;
    double x = 0.0;
    for (uint32_t i = a->n; i; i--) x = ldexp(x, 32) + a->limb[i - 1];
    return a->sign < 0 ? -x : x;
}
int64_t zap_bigint_to_i64(void *p) {
    const ZapBigInt *a = (const ZapBigInt *)p;
    if (a->n > 2 || (a->n == 2 &&
        ((a->sign >= 0 && a->limb[1] > 0x7fffffffU) ||
         (a->sign < 0 && (a->limb[1] > 0x80000000U ||
          (a->limb[1] == 0x80000000U && a->limb[0] != 0)))))) {
        zap_panic(zap_str_new("bigint does not fit in int64", 28));
    }
    uint64_t mag = a->n ? a->limb[0] : 0;
    if (a->n > 1) mag |= (uint64_t)a->limb[1] << 32;
    if (a->sign < 0) return mag == (1ULL << 63) ? INT64_MIN : -(int64_t)mag;
    return (int64_t)mag;
}
int32_t zap_bigint_to_i32(void *p) {
    const ZapBigInt *a = (const ZapBigInt *)p;
    if (a->n > 1 || (a->n == 1 &&
        ((a->sign >= 0 && a->limb[0] > INT32_MAX) ||
         (a->sign < 0 && a->limb[0] > (uint32_t)INT32_MAX + 1u)))) {
        zap_panic(zap_str_new("bigint does not fit in int32", 28));
    }
    uint32_t mag = a->n ? a->limb[0] : 0;
    if (a->sign < 0) return mag == 0x80000000u ? INT32_MIN : -(int32_t)mag;
    return (int32_t)mag;
}
int64_t zap_bigint_cmp(void *ap, void *bp) {
    const ZapBigInt *a = (const ZapBigInt *)ap;
    const ZapBigInt *b = (const ZapBigInt *)bp;
    if (a->sign != b->sign) return a->sign < b->sign ? -1 : 1;
    return a->sign < 0 ? -zap_big_cmp_abs(a, b) : zap_big_cmp_abs(a, b);
}
void *zap_bigint_neg(void *p) {
    const ZapBigInt *a = (const ZapBigInt *)p;
    return zap_big_make(-a->sign, a->limb, a->n);
}
void *zap_bigint_add(void *ap, void *bp) {
    const ZapBigInt *a = (const ZapBigInt *)ap, *b = (const ZapBigInt *)bp;
    uint32_t cap = (a->n > b->n ? a->n : b->n) + 1;
    uint32_t *out = (uint32_t *)calloc(cap, sizeof(uint32_t)); if (!out) abort();
    uint32_t n = a->n;
    if (!a->sign) { memcpy(out,b->limb,b->n*4); n=b->n; }
    else if (!b->sign) memcpy(out,a->limb,a->n*4);
    else if (a->sign && b->sign && a->sign == b->sign) {
        memcpy(out,a->limb,a->n*4); n=a->n;
        zap_big_raw_add(out,&n,b->limb,b->n,cap);
    } else if (a->sign && b->sign && a->sign != b->sign) {
        int c = zap_big_cmp_abs(a,b);
        const ZapBigInt *big = c >= 0 ? a : b, *small = c >= 0 ? b : a;
        memcpy(out,big->limb,big->n*4); n=big->n;
        zap_big_raw_sub(out,&n,small->limb,small->n);
    }
    int sign = !a->sign ? b->sign : !b->sign ? a->sign :
               a->sign == b->sign ? a->sign : zap_big_cmp_abs(a,b) >= 0 ? a->sign : b->sign;
    ZapBigInt *v = zap_big_make(sign,out,n); free(out); return v;
}
void *zap_bigint_sub(void *ap, void *bp) {
    const ZapBigInt *b = (const ZapBigInt *)bp;
    /* The operand is immutable; subtract through magnitude/sign cases. */
    const ZapBigInt *a = (const ZapBigInt *)ap;
    uint32_t cap = (a->n > b->n ? a->n : b->n) + 1;
    uint32_t *out=(uint32_t *)calloc(cap,sizeof(uint32_t)); if(!out) abort();
    uint32_t n=a->n;
    int bs=-b->sign;
    if(!a->sign){memcpy(out,b->limb,b->n*4);n=b->n;}
    else if(!b->sign)memcpy(out,a->limb,a->n*4);
    else if(a->sign==bs){memcpy(out,a->limb,a->n*4);zap_big_raw_add(out,&n,b->limb,b->n,cap);}
    else {int c=zap_big_cmp_abs(a,b);const ZapBigInt*big=c>=0?a:b,*small=c>=0?b:a;memcpy(out,big->limb,big->n*4);n=big->n;zap_big_raw_sub(out,&n,small->limb,small->n);}
    int sign=!a->sign?bs:!b->sign?a->sign:a->sign==bs?a->sign:zap_big_cmp_abs(a,b)>=0?a->sign:bs;
    ZapBigInt*v=zap_big_make(sign,out,n);free(out);return v;
}
void *zap_bigint_mul(void *ap, void *bp) {
    const ZapBigInt *a=(const ZapBigInt*)ap,*b=(const ZapBigInt*)bp;
    ZapBigRaw r=zap_big_raw_mul(a->limb,a->n,b->limb,b->n);
    ZapBigInt*v=zap_big_make(a->sign*b->sign,r.limb,r.n);free(r.limb);return v;
}
static void zap_big_divmod_signed(const ZapBigInt*a,const ZapBigInt*b,ZapBigRaw*q,ZapBigRaw*r,int*qs,int*rs){
    if(!b->sign)zap_panic_typed(ZAP_EXC_ZERO_DIVISION_ERROR,zap_str_new("integer division or modulo by zero",34));
    zap_big_raw_divmod(a->limb,a->n,b->limb,b->n,q,r);
    *qs=a->sign*b->sign;*rs=a->sign;
    if(a->sign!=b->sign&&r->n){
        /* floor quotient: |q| += 1, |r| = |b| - |r|; remainder follows b. */
        uint64_t carry=1;for(uint32_t i=0;i<q->n&&carry;i++){uint64_t x=(uint64_t)q->limb[i]+carry;q->limb[i]=(uint32_t)x;carry=x>>32;}
        if(carry){q->limb=(uint32_t*)realloc(q->limb,(q->n+1)*4);q->limb[q->n++]=(uint32_t)carry;}
        uint32_t *tmp=(uint32_t*)calloc(b->n+1,4);if(!tmp)abort();memcpy(tmp,b->limb,b->n*4);uint32_t tn=b->n;zap_big_raw_sub(tmp,&tn,r->limb,r->n);free(r->limb);r->limb=tmp;r->n=tn;*rs=b->sign;
    }else *rs=b->sign;
}
void *zap_bigint_floordiv(void *ap,void *bp){const ZapBigInt*a=ap,*b=bp;ZapBigRaw q,r;int qs,rs;zap_big_divmod_signed(a,b,&q,&r,&qs,&rs);ZapBigInt*v=zap_big_make(qs,q.limb,q.n);free(q.limb);free(r.limb);return v;}
void *zap_bigint_mod(void *ap,void *bp){const ZapBigInt*a=ap,*b=bp;ZapBigRaw q,r;int qs,rs;zap_big_divmod_signed(a,b,&q,&r,&qs,&rs);ZapBigInt*v=zap_big_make(rs,r.limb,r.n);free(q.limb);free(r.limb);return v;}
void *zap_bigint_pow(void *bp,void *ep){const ZapBigInt*b=bp,*e=ep;if(e->sign<0){if(!b->sign)zap_panic(zap_str_new("zero cannot be raised to a negative power",41));if(b->n==1&&b->limb[0]==1)return zap_bigint_from_i64(b->sign<0&&(e->limb[0]&1)?-1:1);return zap_bigint_from_i64(b->sign<0&&(e->limb[0]&1)?-1:0);}ZapBigRaw acc={1,calloc(1,4)},base=zap_big_raw_copy(b);if(!acc.limb||!base.limb)abort();acc.limb[0]=1;for(uint32_t i=0;i<e->n;i++)for(uint32_t bit=0;bit<32;bit++){if((e->limb[i]>>bit)&1){ZapBigRaw m=zap_big_raw_mul(acc.limb,acc.n,base.limb,base.n);free(acc.limb);acc=m;}if(i+1<e->n||bit<31){ZapBigRaw m=zap_big_raw_mul(base.limb,base.n,base.limb,base.n);free(base.limb);base=m;}}ZapBigInt*v=zap_big_make(b->sign<0&&(e->n&&e->limb[0]&1)?-1:1,acc.limb,acc.n);free(acc.limb);free(base.limb);return v;}
int64_t zap_bigint_truthy(void*p){return ((ZapBigInt*)p)->sign!=0;}
void *zap_bigint_to_str(void*p){const ZapBigInt*a=p;if(!a->sign)return zap_str_new("0",1);ZapBigRaw r=zap_big_raw_copy(a);uint32_t *chunks=(uint32_t*)calloc(r.n*2+1,4);if(!chunks)abort();uint32_t cn=0;while(r.n){uint64_t rem=0;for(uint32_t i=r.n;i;i--){uint64_t x=(rem<<32)|r.limb[i-1];r.limb[i-1]=(uint32_t)(x/1000000000u);rem=x%1000000000u;}zap_big_trim(&r);chunks[cn++]=(uint32_t)rem;}size_t cap=(size_t)cn*9+2;char*buf=(char*)malloc(cap);if(!buf)abort();size_t n=0;if(a->sign<0)buf[n++]='-';n+=(size_t)snprintf(buf+n,cap-n,"%u",chunks[cn-1]);for(uint32_t i=cn-1;i;i--)n+=(size_t)snprintf(buf+n,cap-n,"%09u",chunks[i-1]);void*s=zap_str_new(buf,(int64_t)n);free(buf);free(chunks);free(r.limb);return s;}

void *zap_str_concat(void *a, void *b) {
    ZapStr *x = (ZapStr *)a;
    ZapStr *y = (ZapStr *)b;
    int64_t n = x->len + y->len;
    /* Allocation never collects mid-call, so x/y stay put while we copy (#837). */
    ZapStr *s = zap_str_alloc(n);
    memcpy(s->data, x->data, (size_t)x->len);
    memcpy(s->data + x->len, y->data, (size_t)y->len);
    return s;
}

/* Single-pass join of `n` strings into one heap `ZapStr` (#839; folds in the
 * #837 lever-3(b) f-string join: one allocation for the whole fold, not one per
 * pairwise concat). Distinct from `zap_str_join` (the `str.join(list)` method
 * below). Allocation never collects mid-call, so the `parts` pointers stay put
 * while we copy. `n == 0` yields the empty string. */
void *zap_strbuild(ZapStr **parts, int64_t n) {
    int64_t total = 0;
    for (int64_t i = 0; i < n; i++) {
        total += parts[i]->len;
    }
    ZapStr *s = zap_str_alloc(total);
    char *dst = s->data;
    for (int64_t i = 0; i < n; i++) {
        int64_t l = parts[i]->len;
        memcpy(dst, parts[i]->data, (size_t)l);
        dst += l;
    }
    return s;
}

/* Escape-analysis fast path (#839): build the joined string into a caller-
 * provided inline stack buffer `buf` of `cap` payload bytes (the one-object
 * `ZapStr {int64 len; char data[]}` layout, #838) when the summed length fits,
 * returning `buf` itself — no GC heap allocation. When it does not fit, spill to
 * a normal heap `zap_strbuild`. Pointer-free/atomic and never relocated, exactly
 * like the `ZAP_STACK_STR` borrowed-string helper above; safe because the result
 * is proven not to escape its creating function. */
void *zap_strbuild_into(void *buf, int64_t cap, ZapStr **parts, int64_t n) {
    int64_t total = 0;
    for (int64_t i = 0; i < n; i++) {
        total += parts[i]->len;
    }
    if (total > cap) {
        return zap_strbuild(parts, n);
    }
    ZapStr *s = (ZapStr *)buf;
    s->len = total;
    char *dst = s->data;
    for (int64_t i = 0; i < n; i++) {
        int64_t l = parts[i]->len;
        memcpy(dst, parts[i]->data, (size_t)l);
        dst += l;
    }
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
        /* Integer `**` with a negative exponent has no integer result (the
         * true value is fractional). Rather than silently floor to 0, raise a
         * catchable ValueError; use a float base (`2.0 ** -1`) for a real
         * power. Type-honest: `int ** int` stays `int` (epic #374, B2;
         * decision tracked in #743). */
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
            zap_str_new("negative exponent not supported for integer **; use a float base", 64));
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

/* Forward declaration: `zap_panic` (the `raise`/unwind entry point) is defined
 * further down with the exception glue, but the built-in fault paths above it —
 * integer divide/modulo by zero (EXC-3) and the collection out-of-range faults
 * (#398/#383) — route their faults through it so `except` can catch them. */
void zap_panic(void *exc);

/* Floor division for signed integers (Python semantics, D-0024): rounds toward
 * negative infinity. C `/` truncates toward zero, so correct the quotient when
 * the remainder is non-zero and the operand signs differ. Division by zero is a
 * **catchable** built-in fault (EXC-3, D-0007): it unwinds through `zap_panic`
 * like `raise`, so a `try`/`except` around the `//` catches it (the lowering
 * emits an `invoke`, mirroring the index faults of #398/#383). */
int64_t zap_ifloordiv(int64_t a, int64_t b) {
    if (b == 0) {
        zap_panic_typed(ZAP_EXC_ZERO_DIVISION_ERROR, zap_str_new("integer division or modulo by zero", 34));
    }
    if (a == INT64_MIN && b == -1) {
        /* The true quotient (+2^63) is unrepresentable in int64 and the
         * hardware `idiv` traps (SIGFPE) on this case. Wrap to INT64_MIN,
         * consistent with Zap's wrapping int64 (D-0019), instead of aborting
         * the process (epic #374, issue #742). */
        return INT64_MIN;
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

/* Floored modulo for signed integers (Python semantics, D-0024): the result
 * takes the sign of the divisor, consistent with `zap_ifloordiv` so that the
 * identity `a == (a // b) * b + (a % b)` holds. C `%` truncates toward zero
 * (sign of the dividend), so correct the remainder when it is non-zero and the
 * operand signs differ. Modulo by zero is a **catchable** built-in fault (EXC-3,
 * D-0007) raised with the same message as floor division. */
int64_t zap_imod(int64_t a, int64_t b) {
    if (b == 0) {
        zap_panic_typed(ZAP_EXC_ZERO_DIVISION_ERROR, zap_str_new("integer division or modulo by zero", 34));
    }
    if (b == -1) {
        /* `a % -1 == 0` for every `a`; special-cased so that INT64_MIN % -1
         * does not trap the hardware `idiv` (SIGFPE). Wrapping int64 (D-0019,
         * epic #374, issue #742). */
        return 0;
    }
    int64_t r = a % b;
    if (r != 0 && ((r < 0) != (b < 0))) {
        r += b;
    }
    return r;
}

/* True (float) division. Division by zero raises a catchable ZeroDivisionError
 * (CPython parity, #374/#743) instead of yielding IEEE inf/nan. The lowering
 * emits a plain inline `fdiv` when the divisor is a known-nonzero literal, so
 * this helper is called only when the divisor is not a safe constant. */
double zap_fdiv(double a, double b) {
    if (b == 0.0) {
        zap_panic_typed(ZAP_EXC_ZERO_DIVISION_ERROR, zap_str_new("float division by zero", 22));
    }
    return a / b;
}

/* Floored float modulo (Python semantics, #374/#751): the result takes the sign
 * of the divisor, consistent with floored float `//` (`zap_ffloordiv`), so the
 * identity `a == (a // b) * b + (a % b)` holds for floats as it does for ints
 * (`zap_imod`). C `fmod` truncates toward zero (sign of the dividend), so correct
 * the remainder when it is non-zero and the operand signs differ; when it is
 * zero, give the zero the sign of the divisor (CPython's `float_rem`). Modulo by
 * zero is a catchable ZeroDivisionError (EXC-3, D-0007), like float division by
 * zero. (nan/inf operands fall through `fmod` unchanged — the sign comparison is
 * false for a nan remainder, so it is returned as-is.) */
double zap_ffmod(double a, double b) {
    if (b == 0.0) {
        zap_panic_typed(ZAP_EXC_ZERO_DIVISION_ERROR, zap_str_new("float modulo", 12));
    }
    double r = fmod(a, b);
    if (r != 0.0) {
        if ((b < 0.0) != (r < 0.0)) {
            r += b;
        }
    } else {
        r = copysign(0.0, b);
    }
    return r;
}

/* Left shift with a validated count (#374/#740). A negative count raises a
 * catchable ValueError (CPython). Zap's int is a wrapping int64, so a count >=
 * 64 shifts every bit out and is defined as 0 rather than being LLVM poison.
 * The lowering emits a plain inline shift when the count is a literal in 0..63,
 * so this helper is called only for a non-constant / out-of-range count. The
 * in-range shift is done on the unsigned bit-pattern to avoid signed-shift UB. */
int64_t zap_ishl(int64_t a, int64_t count) {
    if (count < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new("negative shift count", 20));
    }
    if (count >= 64) {
        return 0;
    }
    return (int64_t)((uint64_t)a << (uint64_t)count);
}

/* Arithmetic right shift with a validated count (#374/#740). A negative count
 * raises a catchable ValueError (CPython). A count >= 64 saturates to the sign
 * bit (0 for a non-negative value, -1 for a negative one) — the limit of an
 * arithmetic shift under wrapping int64. */
int64_t zap_ishr(int64_t a, int64_t count) {
    if (count < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new("negative shift count", 20));
    }
    if (count >= 64) {
        return a < 0 ? -1 : 0;
    }
    return a >> count;
}

/* Unsigned integer helpers (#552). The i64-word operands carry unsigned values
 * (lowering promotes unsigned arithmetic to a 64-bit width and wraps modulo
 * 2^64), so each reinterprets its words as `uint64_t`. Divide/modulo by zero is
 * the same catchable ZeroDivisionError as the signed path; there is no INT64_MIN
 * overflow case for unsigned division. `//` is a plain truncating `udiv` (floor
 * == truncation when both operands are non-negative) and `%` a plain `urem`. */
int64_t zap_udiv(int64_t a, int64_t b) {
    uint64_t ua = (uint64_t)a, ub = (uint64_t)b;
    if (ub == 0) {
        zap_panic_typed(ZAP_EXC_ZERO_DIVISION_ERROR, zap_str_new("integer division or modulo by zero", 34));
    }
    return (int64_t)(ua / ub);
}
int64_t zap_umod(int64_t a, int64_t b) {
    uint64_t ua = (uint64_t)a, ub = (uint64_t)b;
    if (ub == 0) {
        zap_panic_typed(ZAP_EXC_ZERO_DIVISION_ERROR, zap_str_new("integer division or modulo by zero", 34));
    }
    return (int64_t)(ua % ub);
}
/* Unsigned power by binary exponentiation, wrapping modulo 2^64 (#552). The
 * exponent is unsigned, so there is no negative-exponent case (unlike
 * `zap_ipow`); it cannot raise. */
int64_t zap_upow(int64_t base, int64_t exp) {
    uint64_t result = 1, b = (uint64_t)base, e = (uint64_t)exp;
    while (e != 0) {
        if (e & 1u) {
            result *= b;
        }
        e >>= 1;
        if (e != 0) {
            b *= b;
        }
    }
    return (int64_t)result;
}
/* Logical right shift with a validated count (#552), the unsigned counterpart of
 * `zap_ishr`: zero-fills from the left. A negative count raises a catchable
 * ValueError; a count >= 64 shifts every bit out and is defined as 0 (not LLVM
 * poison), consistent with `zap_ishl`. */
int64_t zap_ushr(int64_t a, int64_t count) {
    if (count < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new("negative shift count", 20));
    }
    if (count >= 64) {
        return 0;
    }
    return (int64_t)((uint64_t)a >> (uint64_t)count);
}
/* Clamp an unsigned subscript index to the signed index word (#552). An
 * unsigned index zero-extends, so it is non-negative by construction: a small
 * width (`uint8`/`uint16`/`uint32`) always fits in `[0, 2^32)` and reaches a
 * collection's bounds check unchanged. Only a `uint64`/`usize` can exceed
 * `INT64_MAX`; such a value would reinterpret to a negative `int64` and be
 * mistaken for a from-the-end index (`xs[-1]`). No real collection has `2^63`
 * elements, so any such value is out of range: clamp it to `INT64_MAX`, which
 * is `>=` every possible length, so the collection's own bounds check raises
 * `IndexError` instead of silently wrapping from the end. This helper never
 * raises; the raise happens in the normal bounds check downstream. */
int64_t zap_uindex(int64_t i) {
    uint64_t u = (uint64_t)i;
    return u > (uint64_t)INT64_MAX ? INT64_MAX : (int64_t)u;
}

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

/* AtomicArray[T] (#844, D-01xx): a fixed-capacity array of atomically CAS-able
 * machine-word slots, the pointer-array primitive the field-based `Atomic[T]`
 * (D-0066) could not provide. Each slot is one 8-byte word; `load`/`store`/`cas`
 * are seq_cst (`__atomic_*`), matching `Atomic[T]`. `elem_ptr` records whether
 * the slots hold GC references (set from the monomorphized element type), so the
 * collector traces + forwards each slot exactly like a `list[T]` of pointers
 * (ZAP_TD_ATOMICARRAY trace in zapgc.inc.c). The `slots` buffer is allocated with
 * `zap_ptrbuf` (pointer-scanned under conservative Boehm, precisely traced under
 * moving Immix) so stored references stay alive under BOTH collectors — the
 * correctness invariant from D-0078 §4.4 (every stored pointer in a genuine
 * reference slot, never a packed integer). Capacity is fixed at construction;
 * there is no growth, so a slot address is stable across the lifetime (only the
 * buffer itself may be relocated at STW, between atomic ops, which reload it). */
typedef struct {
    int64_t cap;      /* number of slots (fixed)                               */
    int64_t *slots;   /* `cap` word slots; CAS-able; may hold GC pointers      */
    uint8_t elem_ptr; /* 1 iff slots hold GC references (traced + forwarded)   */
} ZapAtomicArray;

void *zap_atomicarray_new(int64_t cap, int64_t elem_is_ptr) {
    if (cap < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("AtomicArray capacity must be >= 0", 33));
    }
    ZapAtomicArray *a = (ZapAtomicArray *)zap_obj(&zap_desc_atomicarray);
    a->cap = cap;
    a->elem_ptr = elem_is_ptr ? 1 : 0;
    /* zero-init: unused slots read as 0 / None. `zap_ptrbuf` (GC_MALLOC_ATOMIC
     * under Immix) does not guarantee zeroed memory, so memset explicitly. */
    int64_t *slots = (int64_t *)zap_ptrbuf(cap * (int64_t)sizeof(int64_t));
    if (cap > 0) {
        memset(slots, 0, (size_t)cap * sizeof(int64_t));
    }
    a->slots = slots;
    return a;
}

int64_t zap_atomicarray_len(void *ap) { return ((ZapAtomicArray *)ap)->cap; }

/* Out-of-range access raises a catchable Zap exception (#383), like list
 * indexing. The index is reported as written. */
static void zap_atomicarray_bounds(ZapAtomicArray *a, int64_t i) {
    if (i < 0 || i >= a->cap) {
        char msg[96];
        int n = snprintf(msg, sizeof(msg),
                         "AtomicArray index %lld out of range (len %lld)",
                         (long long)i, (long long)a->cap);
        zap_panic_typed(ZAP_EXC_INDEX_ERROR,
                        zap_str_new(msg, n < 0 ? 0 : (int64_t)n));
    }
}

/* Core i64-word ops. The reference (`_ptr`) entry points below are thin bitwise
 * wrappers (a GC pointer is one machine word), kept as separate symbols so
 * lowering can pass/return a `ptr` value with no ptr<->int cast at the IR
 * boundary — the same typed-entry-point split channel send/recv uses
 * (`zap_chan_send_i64` vs `_ptr`). */
int64_t zap_atomicarray_load_i64(void *ap, int64_t i) {
    ZapAtomicArray *a = (ZapAtomicArray *)ap;
    zap_atomicarray_bounds(a, i);
    return __atomic_load_n(&a->slots[i], __ATOMIC_SEQ_CST);
}

void zap_atomicarray_store_i64(void *ap, int64_t i, int64_t word) {
    ZapAtomicArray *a = (ZapAtomicArray *)ap;
    zap_atomicarray_bounds(a, i);
    __atomic_store_n(&a->slots[i], word, __ATOMIC_SEQ_CST);
}

/* seq_cst compare-and-swap: store `desired` iff slot i currently equals
 * `expected`; return 1 on success, 0 otherwise. `__atomic_compare_exchange_n`
 * writes the observed value back into its expected arg on failure, so a local
 * copy is used to leave the caller's value untouched. */
int64_t zap_atomicarray_cas_i64(void *ap, int64_t i, int64_t expected,
                                int64_t desired) {
    ZapAtomicArray *a = (ZapAtomicArray *)ap;
    zap_atomicarray_bounds(a, i);
    int64_t exp = expected;
    /* `weak` = 0 (strong CAS); the runtime C has no <stdbool.h>, so a plain 0. */
    return __atomic_compare_exchange_n(&a->slots[i], &exp, desired, 0,
                                       __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST)
               ? 1
               : 0;
}

void *zap_atomicarray_load_ptr(void *ap, int64_t i) {
    return (void *)(intptr_t)zap_atomicarray_load_i64(ap, i);
}

void zap_atomicarray_store_ptr(void *ap, int64_t i, void *v) {
    zap_atomicarray_store_i64(ap, i, (int64_t)(intptr_t)v);
}

int64_t zap_atomicarray_cas_ptr(void *ap, int64_t i, void *expected,
                                void *desired) {
    return zap_atomicarray_cas_i64(ap, i, (int64_t)(intptr_t)expected,
                                   (int64_t)(intptr_t)desired);
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

/* Normalize a (possibly negative) list index and bounds-check it, returning the
 * resolved non-negative index. Negative indices count from the end (`xs[-1]` is
 * the last element), mirroring strings (`zap_str_index`, #398) and CPython.
 *
 * Out-of-range access raises a catchable Zap exception (#383): the fault
 * unwinds through `zap_panic` like `raise`, so a `try/except` around the
 * indexing intercepts it (and an uncaught one still terminates non-zero via
 * `zap_uncaught`). Matches D-0007 ("built-in faults ... try/except handles
 * them") and CPython's `IndexError`. The error reports the index as written. */
static int64_t zap_list_norm(ZapList *l, int64_t i) {
    int64_t orig = i;
    if (i < 0) {
        i += l->len;
    }
    if (i < 0 || i >= l->len) {
        char buf[96];
        int n = snprintf(buf, sizeof buf, "list index %lld out of range (len %lld)",
                         (long long)orig, (long long)l->len);
        if (n < 0) {
            n = 0;
        }
        if (n >= (int)sizeof buf) {
            n = (int)sizeof buf - 1;
        }
        zap_panic_typed(ZAP_EXC_INDEX_ERROR, zap_str_new(buf, (int64_t)n));
    }
    return i;
}

static int64_t zap_list_at(ZapList *l, int64_t i) {
    return l->data[zap_list_norm(l, i)];
}

void zap_list_push_i64(void *l, int64_t v) { zap_list_push((ZapList *)l, v); }
void zap_list_push_ptr(void *l, void *v) {
    ((ZapList *)l)->elem_ptr = 1; /* precise GC: elements are pointers (#48) */
    zap_list_push((ZapList *)l, (int64_t)(intptr_t)v);
}
/* `list[float]` (I#569): a `double` element shares the uniform 8-byte word, so
 * it is stored by reinterpreting its bits as the `int64_t` word (as the REPL
 * float slot does, I#252). The elements are not GC pointers, so `elem_ptr` stays
 * 0 and the collector never follows them. */
void zap_list_push_f64(void *l, double v) {
    union {
        int64_t i;
        double d;
    } u;
    u.d = v;
    zap_list_push((ZapList *)l, u.i);
}
int64_t zap_list_get_i64(void *l, int64_t i) { return zap_list_at((ZapList *)l, i); }
void *zap_list_get_ptr(void *l, int64_t i) {
    return (void *)(intptr_t)zap_list_at((ZapList *)l, i);
}
double zap_list_get_f64(void *l, int64_t i) {
    union {
        int64_t i;
        double d;
    } u;
    u.i = zap_list_at((ZapList *)l, i);
    return u.d;
}
void zap_list_set_i64(void *l, int64_t i, int64_t v) {
    int64_t k = zap_list_norm((ZapList *)l, i); /* normalize + bounds-check */
    ((ZapList *)l)->data[k] = v;
}
void zap_list_set_ptr(void *l, int64_t i, void *v) {
    ((ZapList *)l)->elem_ptr = 1;
    int64_t k = zap_list_norm((ZapList *)l, i); /* normalize + bounds-check */
    ((ZapList *)l)->data[k] = (int64_t)(intptr_t)v;
}
void zap_list_set_f64(void *l, int64_t i, double v) {
    union {
        int64_t i;
        double d;
    } u;
    u.d = v;
    int64_t k = zap_list_norm((ZapList *)l, i); /* normalize + bounds-check */
    ((ZapList *)l)->data[k] = u.i;
}
int64_t zap_list_len(void *l) { return ((ZapList *)l)->len; }

/* Reinterpret a `double` as the uniform 8-byte collection word and back (#800).
 * A float dict value, channel element, or heap element shares the same storage
 * word as an `i64` element (as `list[float]` does, I#569); these helpers carry
 * the bits across without changing the value, and the elements are not GC
 * pointers so the collector never follows them. */
static int64_t zap_f64_bits(double v) {
    union {
        int64_t i;
        double d;
    } u;
    u.d = v;
    return u.i;
}
static double zap_f64_from_bits(int64_t w) {
    union {
        int64_t i;
        double d;
    } u;
    u.i = w;
    return u.d;
}

/* Canonicalize a `float64` dict KEY / set MEMBER and reject NaN (#803, D-0138).
 * Zap floats are value words with no object identity, so Python's identity-based
 * `nan`-as-key behaviour cannot be reproduced; any NaN used as a key/member
 * therefore raises a CATCHABLE `ValueError` (via `zap_panic_typed`, the same
 * mechanism as float division-by-zero), never a silent slot or a process abort.
 * A finite key is canonicalized `-0.0 -> +0.0` so `0.0` and `-0.0` collide to one
 * slot (Python `==` parity). The returned word is the canonical IEEE bits, which
 * the dict/set then store, hash, and compare as a plain `i64` word (KEY_I64):
 * after canonicalization and NaN rejection, float equality IS bit equality, so no
 * new keykind or per-element comparator is needed. `msg`/`len` name the position
 * ("...dict key" / "...set member") for the raised exception. */
static int64_t zap_f64_key_word(double v, const char *msg, int64_t len) {
    if (isnan(v)) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(msg, len));
    }
    if (v == 0.0) {
        v = 0.0; /* collapse -0.0 to +0.0 (both compare == 0.0) */
    }
    return zap_f64_bits(v);
}

/* Fixed messages for the two NaN-rejection positions (#803). */
#define ZAP_F64_DICT_KEY_NAN "NaN cannot be used as a dict key"
#define ZAP_F64_SET_MEM_NAN "NaN cannot be used as a set member"

/* Hash-key canonicalization for a `float`/`float64` FIELD of a value aggregate
 * (value struct / tuple) used as a dict key or set member (D-0139/#808). The
 * aggregate's auto-derived `__hash__` composes its fields' hashes; a float field
 * inherits the #803 key semantics — `-0.0` canonicalized to `+0.0` and `NaN`
 * rejected as a catchable `ValueError` — exactly as a bare float key does, so two
 * aggregates equal under the derived `__eq__` (where `-0.0 == 0.0`) hash equal.
 * Non-`static`: the generated `__hash__` calls it by name. */
int64_t zap_f64_value_key_word(double v) {
    return zap_f64_key_word(v, ZAP_F64_DICT_KEY_NAN,
                            (int64_t)sizeof(ZAP_F64_DICT_KEY_NAN) - 1);
}

/* `del xs[i]` (#595): remove the element at index `i`, shifting the tail down
 * one slot. The index is normalized (negatives count from the end) and bounds-
 * checked by `zap_list_norm`, so an out-of-range `del` raises the same catchable
 * `IndexError` as a read `xs[i]`. Element-type-agnostic: it moves uniform 8-byte
 * words, so it serves every `list[T]` (the precise collector still sees only the
 * live prefix because `len` shrinks and the vacated tail word is cleared). */
void zap_list_del_at(void *lp, int64_t i) {
    ZapList *l = (ZapList *)lp;
    int64_t k = zap_list_norm(l, i); /* normalize + bounds-check (IndexError) */
    for (int64_t j = k; j + 1 < l->len; j++) {
        l->data[j] = l->data[j + 1];
    }
    l->len--;
    l->data[l->len] = 0; /* don't pin the removed referent for the collector */
}

/* Insert a word at index `i`, shifting the tail right (#977). `i` is clamped to
 * `[0, len]` — a negative index counts from the end, and both ends saturate —
 * so `insert` never raises (CPython `list.insert`). The typed wrappers below
 * mirror `zap_list_push_*`. */
static void zap_list_insert_word(ZapList *l, int64_t i, int64_t word) {
    if (i < 0) {
        i += l->len;
        if (i < 0) {
            i = 0;
        }
    }
    if (i > l->len) {
        i = l->len;
    }
    if (l->len == l->cap) {
        zap_list_grow(l);
    }
    for (int64_t j = l->len; j > i; j--) {
        l->data[j] = l->data[j - 1];
    }
    l->data[i] = word;
    l->len++;
}
void zap_list_insert_i64(void *lp, int64_t i, int64_t v) {
    zap_list_insert_word((ZapList *)lp, i, v);
}
void zap_list_insert_ptr(void *lp, int64_t i, void *v) {
    ((ZapList *)lp)->elem_ptr = 1; /* precise GC: elements are pointers (#48) */
    zap_list_insert_word((ZapList *)lp, i, (int64_t)(intptr_t)v);
}
void zap_list_insert_f64(void *lp, int64_t i, double v) {
    union {
        int64_t i;
        double d;
    } u;
    u.d = v;
    zap_list_insert_word((ZapList *)lp, i, u.i);
}

/* Remove every element (#977, CPython `list.clear`); the freed slots are zeroed
 * so the precise collector does not pin removed referents. */
void zap_list_clear(void *lp) {
    ZapList *l = (ZapList *)lp;
    for (int64_t j = 0; j < l->len; j++) {
        l->data[j] = 0;
    }
    l->len = 0;
}

/* Shallow copy: a fresh list with the same element words (shared referents) and
 * the same `elem_ptr` kind, so the precise collector traces the copy correctly.
 * Used by `sorted()` (ticket #336) to leave its input untouched. */
void *zap_list_copy(void *lp) {
    ZapList *l = (ZapList *)lp;
    ZapList *o = (ZapList *)zap_list_new();
    if (l->len > 0) {
        o->data = (int64_t *)zap_ptrbuf((int64_t)l->len * (int64_t)sizeof(int64_t));
        memcpy(o->data, l->data, (size_t)l->len * sizeof(int64_t));
        o->len = l->len;
        o->cap = l->len;
        o->elem_ptr = l->elem_ptr;
    }
    return o;
}

/* list.pop([i]) (#1095): remove and RETURN the element at index `i`, shifting
 * the tail down one slot (CPython `list.pop`). `has_index == 0` pops the LAST
 * element in O(1) (the loop below runs zero times — no shift). A negative index
 * counts from the end. An out-of-range index, or a pop from an empty list,
 * raises a CATCHABLE `IndexError` (via `zap_list_norm` for the indexed form, or
 * the explicit empty check below) — so the lowering MUST `invoke` these so a
 * `try`/`except` can catch it. Element-type-agnostic: it moves uniform 8-byte
 * words, and the vacated tail slot is zeroed so the precise collector does not
 * pin the removed referent. The returned word is handed back through the typed
 * wrappers below, mirroring `zap_list_get_*`, so every `list[T]` element kind
 * (int, bytes/str, pointer/struct reference, float) round-trips with no bit
 * change. This helper is a GC leaf (no alloc on the success path, no call into
 * compiled Zap), so the element words stay rooted via the live list. */
static int64_t zap_list_pop_word(ZapList *l, int64_t i, int64_t has_index) {
    int64_t k;
    if (has_index) {
        k = zap_list_norm(l, i); /* normalize + bounds-check (IndexError) */
    } else {
        if (l->len == 0) {
            zap_panic_typed(ZAP_EXC_INDEX_ERROR,
                            zap_str_new("pop from empty list", 19));
        }
        k = l->len - 1;
    }
    int64_t w = l->data[k];
    for (int64_t j = k; j + 1 < l->len; j++) {
        l->data[j] = l->data[j + 1];
    }
    l->len--;
    l->data[l->len] = 0; /* don't pin the removed referent for the collector */
    return w;
}
int64_t zap_list_pop_i64(void *lp, int64_t i, int64_t has_index) {
    return zap_list_pop_word((ZapList *)lp, i, has_index);
}
void *zap_list_pop_ptr(void *lp, int64_t i, int64_t has_index) {
    return (void *)(intptr_t)zap_list_pop_word((ZapList *)lp, i, has_index);
}
double zap_list_pop_f64(void *lp, int64_t i, int64_t has_index) {
    return zap_f64_from_bits(zap_list_pop_word((ZapList *)lp, i, has_index));
}

/* list.reverse() (#1095): reverse the elements in place (CPython
 * `list.reverse`). An element-type-agnostic word swap over the whole list; a GC
 * leaf (no alloc, no call into compiled Zap), so the element words stay rooted
 * via the live list for its duration. `elem_ptr` is unchanged — the same
 * referents, in the opposite order. */
void zap_list_reverse(void *lp) {
    ZapList *l = (ZapList *)lp;
    int64_t a = 0;
    int64_t b = l->len - 1;
    while (a < b) {
        int64_t t = l->data[a];
        l->data[a] = l->data[b];
        l->data[b] = t;
        a++;
        b--;
    }
}

/* list[T] slice `xs[lo:hi:step]` -> a NEW list (a copy, D-0030; contrast array
 * slices, which are views). Python bounds: negatives count from the end, out-of-
 * range clamps (no fault — unlike indexing), and `step` may be negative. The
 * element words are copied verbatim and `elem_ptr` is inherited, so the precise
 * collector traces the copy correctly and the operation is element-type-agnostic
 * (one entry point serves both the i64 and ptr element kinds). The slice-bound
 * clamps (`zap_slice_pos`/`zap_slice_neg`) are shared with `zap_str_slice` and
 * are defined with the string helpers below; forward-declare them here. */
static int64_t zap_slice_pos(int64_t v, int64_t n);
static int64_t zap_slice_neg(int64_t v, int64_t n);
/* `zap_shape_err` is defined with the shape ops (#511) but used earlier by the
 * advanced-indexing slice normalizer (#510); forward-declare it. */
static void zap_shape_err(const char *m);
void *zap_list_slice(void *lp, int64_t lo, int64_t hi, int64_t step, int64_t has_lo,
                     int64_t has_hi, int64_t has_step) {
    ZapList *l = (ZapList *)lp;
    int64_t n = l->len;
    int64_t st = has_step ? step : 1;
    if (st == 0) {
        zap_panic(zap_str_new("slice step cannot be zero", 25));
    }
    int64_t start;
    int64_t stop;
    if (st > 0) {
        start = has_lo ? zap_slice_pos(lo, n) : 0;
        stop = has_hi ? zap_slice_pos(hi, n) : n;
    } else {
        start = has_lo ? zap_slice_neg(lo, n) : n - 1;
        stop = has_hi ? zap_slice_neg(hi, n) : -1;
    }
    ZapList *o = (ZapList *)zap_list_new();
    o->elem_ptr = l->elem_ptr;
    for (int64_t k = start; (st > 0) ? (k < stop) : (k > stop); k += st) {
        zap_list_push(o, l->data[k]);
    }
    return o;
}

/* Sliced assignment `xs[lo:hi:step] = ys` (#1005, Python semantics). The right-
 * hand side `ys` is a list; its elements are snapshotted first so an aliasing
 * assignment (`xs[:] = xs`) and the in-place tail shift are both safe. A step-1
 * slice is a splice — the region [start, stop) is replaced by all of `ys`, so
 * the list may grow or shrink. An extended slice (step != 1) requires the number
 * of selected indices to equal `len(ys)` (else a catchable ValueError) and
 * assigns element-wise. Element words are copied verbatim and the pointer-ness
 * is inherited, so the op is element-kind-agnostic (int64 / ptr / float). No
 * parking and allocation never collects mid-call, so the snapshot and the list's
 * own element words stay valid throughout. */
void zap_list_set_slice(void *lp, void *yp, int64_t lo, int64_t hi, int64_t step,
                        int64_t has_lo, int64_t has_hi, int64_t has_step) {
    ZapList *l = (ZapList *)lp;
    ZapList *ys = (ZapList *)yp;
    int64_t n = l->len;
    int64_t st = has_step ? step : 1;
    if (st == 0) {
        zap_panic(zap_str_new("slice step cannot be zero", 25));
    }
    /* Snapshot the RHS elements up front (aliasing-safe). */
    int64_t m = ys->len;
    int64_t *src = NULL;
    if (m > 0) {
        src = (int64_t *)zap_ptrbuf(m * (int64_t)sizeof(int64_t));
        memcpy(src, ys->data, (size_t)m * sizeof(int64_t));
    }
    if (st == 1) {
        int64_t start = has_lo ? zap_slice_pos(lo, n) : 0;
        int64_t stop = has_hi ? zap_slice_pos(hi, n) : n;
        if (stop < start) {
            stop = start; /* empty region: a pure insertion at `start` */
        }
        int64_t remove = stop - start;
        int64_t newlen = n - remove + m;
        while (l->cap < newlen) {
            zap_list_grow(l);
        }
        /* Shift the tail [stop, n) to sit after the inserted block. */
        if (n - stop > 0) {
            memmove(&l->data[start + m], &l->data[stop],
                    (size_t)(n - stop) * sizeof(int64_t));
        }
        if (m > 0) {
            memcpy(&l->data[start], src, (size_t)m * sizeof(int64_t));
        }
        l->len = newlen;
    } else {
        int64_t start;
        int64_t stop;
        if (st > 0) {
            start = has_lo ? zap_slice_pos(lo, n) : 0;
            stop = has_hi ? zap_slice_pos(hi, n) : n;
        } else {
            start = has_lo ? zap_slice_neg(lo, n) : n - 1;
            stop = has_hi ? zap_slice_neg(hi, n) : -1;
        }
        int64_t count = 0;
        for (int64_t k = start; (st > 0) ? (k < stop) : (k > stop); k += st) {
            count++;
        }
        if (count != m) {
            char msg[128];
            int msglen = snprintf(msg, sizeof(msg),
                                  "attempt to assign sequence of size %lld to extended "
                                  "slice of size %lld",
                                  (long long)m, (long long)count);
            zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(msg, msglen));
        }
        int64_t j = 0;
        for (int64_t k = start; (st > 0) ? (k < stop) : (k > stop); k += st) {
            l->data[k] = src[j++];
        }
    }
    if (ys->elem_ptr && m > 0) {
        l->elem_ptr = 1;
    }
}

/* dict[K, V]: open-addressing hash table with linear probing.
 * Key kind (int64 vs str) is passed per call; a dict is homogeneous, so this is
 * stable across a given dict's lifetime. */
/* KEY_FN (operator overloading, D-0047): a user-struct key, hashed/compared
 * through the type's own `__hash__`/`__eq__`, which the dict/set stores as
 * function pointers. `__hash__(self) -> i64` and `__eq__(a, b) -> bool` match the
 * ZIR-generated signatures (an LLVM `i1` return maps to C `_Bool`). */
enum { KEY_I64 = 0, KEY_STR = 1, KEY_FN = 2 };

typedef int64_t (*ZapHashFn)(void *self);
typedef _Bool (*ZapEqFn)(void *a, void *b);

/* Bucketed / SoA table layout (#840, D-0156). The old flat `ZapEntry[]` dragged
 * a whole 32-byte entry (state + key + val + cached hash) into cache for every
 * probed slot. The table now stores a single `zap_ptrbuf` buffer carved into
 * four parallel regions — a dense one-byte TAG stream, then keys, then values,
 * then cached hashes — so a probe scans only the 1-byte tag stream (64 slots per
 * cache line, SIMD-friendly) and touches a key/val/hash word only on a tag hit.
 *
 * The tag is the top 8 bits of the hash, biased out of the two reserved sentinel
 * values so a real key's tag can never alias EMPTY/TOMB: a mismatching tag
 * rejects a slot with one byte compare, and a tag MATCH always falls through to
 * the definitive compare (the full cached hash gates the str/struct key compare;
 * an int key compares its word directly — equality there needs no hash gate, so
 * the int fast path never reads the `hashes` region at all, #840). */
enum { ZAP_TAG_EMPTY = 0, ZAP_TAG_TOMB = 1 }; /* reserved sentinels; real tag >= 2 */

/* Top-8 hash bits, biased into [2,255] so a real tag never equals a sentinel. */
static inline uint8_t zap_tag(uint64_t h) {
    uint8_t t = (uint8_t)(h >> 56);
    return t < 2 ? (uint8_t)(t + 2) : t;
}

typedef struct {
    int64_t len;
    int64_t cap; /* number of slots; always a power of two or 0 */
    /* ONE buffer: tags[cap] (1B) | keys[cap] (8B) | vals[cap] (8B) | hashes[cap]
     * (8B) = 25*cap bytes. `cap` is a power of two, so every 8-byte region is
     * naturally aligned. NULL while cap == 0. */
    void *buf;
    void *hashfn; /* struct keys only (KEY_FN): the type's __hash__ / __eq__ */
    void *eqfn;
    uint8_t key_ptr; /* keys are GC pointers (str/struct) */
    uint8_t val_ptr; /* values are GC pointers */
} ZapDict;

/* SoA region accessors. Each RE-READS `d->buf` and `d->cap`, so if a KEY_FN
 * `__hash__`/`__eq__` callback relocated the buffer under a moving collector the
 * next access picks up the new address — the same discipline the flat layout got
 * from re-reading `d->entries[j]` each iteration. The callback-free int/str
 * paths have no intervening store, so the optimiser hoists these loads. */
static inline uint8_t *zap_d_tags(const ZapDict *d) { return (uint8_t *)d->buf; }
static inline int64_t *zap_d_keys(const ZapDict *d) {
    return (int64_t *)((char *)d->buf + d->cap);
}
static inline int64_t *zap_d_vals(const ZapDict *d) {
    return (int64_t *)((char *)d->buf + d->cap * 9);
}
static inline uint64_t *zap_d_hashes(const ZapDict *d) {
    return (uint64_t *)((char *)d->buf + d->cap * 17);
}
/* Bytes backing a `cap`-slot table (tags + keys + vals + hashes). */
static inline int64_t zap_d_bytes(int64_t cap) { return cap * 25; }

void *zap_dict_new(void) {
    ZapDict *d = (ZapDict *)zap_obj(&zap_desc_dict);
    d->len = 0;
    d->cap = 0;
    d->buf = NULL;
    d->hashfn = NULL;
    d->eqfn = NULL;
    return d;
}

/* ---------------------------------------------------------------------------
 * wyhash (final v3) — the dict/set string-key hash and the @derive(Hash) /
 * value-struct string-field hash (#1064). Public domain (Wang Yi,
 * https://github.com/wangyi-fudan/wyhash). Vendored as the portable SCALAR
 * implementation only: the 64x64->128 multiply uses `__uint128_t` (supported on
 * x86_64 + aarch64), reads are unaligned-safe via `memcpy`, and there is NO SIMD
 * and NO CPU-feature dispatch — so this is a standalone runtime primitive, NOT
 * under the #139/#947 ISA-accel facility. It replaces the former byte-at-a-time
 * FNV-1a: wyhash keeps FNV's collision quality but drops its serial
 * multiply/xor dependency chain, which measured ~3-5x slower at 16B and up to
 * ~31x at 1KB on the perf box (see #1064).
 * ------------------------------------------------------------------------- */
#if defined(__GNUC__) || defined(__clang__)
#define ZAP_WY_LIKELY(x) __builtin_expect(x, 1)
#define ZAP_WY_UNLIKELY(x) __builtin_expect(x, 0)
#else
#define ZAP_WY_LIKELY(x) (x)
#define ZAP_WY_UNLIKELY(x) (x)
#endif

/* 64x64 -> 128 multiply, folded back into *A (low word) and *B (high word). */
static inline void zap_wymum(uint64_t *A, uint64_t *B) {
#if defined(__SIZEOF_INT128__)
    __uint128_t r = *A;
    r *= *B;
    *A = (uint64_t)r;
    *B = (uint64_t)(r >> 64);
#else
    /* Portable 32x32 fallback (not expected on the supported targets). */
    uint64_t ha = *A >> 32, hb = *B >> 32, la = (uint32_t)*A, lb = (uint32_t)*B;
    uint64_t rh = ha * hb, rm0 = ha * lb, rm1 = hb * la, rl = la * lb;
    uint64_t t = rl + (rm0 << 32), c = t < rl;
    uint64_t lo = t + (rm1 << 32);
    c += lo < t;
    uint64_t hi = rh + (rm0 >> 32) + (rm1 >> 32) + c;
    *A = lo;
    *B = hi;
#endif
}

/* MUM mix: multiply then xor-fold the two halves. */
static inline uint64_t zap_wymix(uint64_t A, uint64_t B) {
    zap_wymum(&A, &B);
    return A ^ B;
}

/* Little-endian reads. The supported targets are little-endian; a big-endian
 * build byte-swaps so a value still hashes identically within its own process,
 * which is all the seed-domain contract requires. */
#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
static inline uint64_t zap_wyr8(const uint8_t *p) {
    uint64_t v;
    memcpy(&v, p, 8);
    return __builtin_bswap64(v);
}
static inline uint64_t zap_wyr4(const uint8_t *p) {
    uint32_t v;
    memcpy(&v, p, 4);
    return __builtin_bswap32(v);
}
#else
static inline uint64_t zap_wyr8(const uint8_t *p) {
    uint64_t v;
    memcpy(&v, p, 8);
    return v;
}
static inline uint64_t zap_wyr4(const uint8_t *p) {
    uint32_t v;
    memcpy(&v, p, 4);
    return v;
}
#endif
static inline uint64_t zap_wyr3(const uint8_t *p, size_t k) {
    return (((uint64_t)p[0]) << 16) | (((uint64_t)p[k >> 1]) << 8) | p[k - 1];
}

/* The default wyhash secret (final v3 constants). */
static const uint64_t zap_wy_secret[4] = {
    0x2d358dccaa6c78a5ull, 0x8bb84b93962eacc9ull, 0x4b33a62ed433d4a3ull,
    0x4d5a2da51de1aa47ull};

static inline uint64_t zap_wyhash(const void *key, size_t len, uint64_t seed) {
    const uint8_t *p = (const uint8_t *)key;
    const uint64_t *secret = zap_wy_secret;
    seed ^= zap_wymix(seed ^ secret[0], secret[1]);
    uint64_t a, b;
    if (ZAP_WY_LIKELY(len <= 16)) {
        if (ZAP_WY_LIKELY(len >= 4)) {
            a = (zap_wyr4(p) << 32) | zap_wyr4(p + ((len >> 3) << 2));
            b = (zap_wyr4(p + len - 4) << 32) |
                zap_wyr4(p + len - 4 - ((len >> 3) << 2));
        } else if (ZAP_WY_LIKELY(len > 0)) {
            a = zap_wyr3(p, len);
            b = 0;
        } else {
            a = b = 0;
        }
    } else {
        size_t i = len;
        if (ZAP_WY_UNLIKELY(i > 48)) {
            uint64_t see1 = seed, see2 = seed;
            do {
                seed =
                    zap_wymix(zap_wyr8(p) ^ secret[1], zap_wyr8(p + 8) ^ seed);
                see1 = zap_wymix(zap_wyr8(p + 16) ^ secret[2],
                                 zap_wyr8(p + 24) ^ see1);
                see2 = zap_wymix(zap_wyr8(p + 32) ^ secret[3],
                                 zap_wyr8(p + 40) ^ see2);
                p += 48;
                i -= 48;
            } while (ZAP_WY_LIKELY(i > 48));
            seed ^= see1 ^ see2;
        }
        while (ZAP_WY_UNLIKELY(i > 16)) {
            seed = zap_wymix(zap_wyr8(p) ^ secret[1], zap_wyr8(p + 8) ^ seed);
            i -= 16;
            p += 16;
        }
        a = zap_wyr8(p + i - 16);
        b = zap_wyr8(p + i - 8);
    }
    a ^= secret[1];
    b ^= seed;
    zap_wymum(&a, &b);
    return zap_wymix(a ^ secret[0] ^ len, b ^ secret[1]);
}

/* Per-process hash seed (#1064): captured once at startup and inherited across
 * `fork` (a C constructor runs per process image, not per fork). Seeding closes
 * the unseeded hash-flooding exposure of the former FNV str hash and makes str
 * hashes differ across independent processes (Python-like) — a persisted str
 * hash is therefore invalid; use a named digest instead. `ZAPHASHSEED=<uint>`
 * pins a fixed seed (0 allowed) for reproducible runs / goldens, à la
 * PYTHONHASHSEED; unset (or empty / "random") keeps a random per-process seed.
 * The fork / exec / multiprocessing seed-domain semantics, and the DECISIONS.md
 * entry, are owned by the maintainer (see #1064). */
static uint64_t zap_hash_seed = 0;

static uint64_t zap_hash_os_seed(void) {
    uint64_t s = 0;
#if defined(__APPLE__)
    arc4random_buf(&s, sizeof s);
#elif defined(__linux__)
    if (getentropy(&s, sizeof s) != 0) {
        s = (uint64_t)time(NULL) ^ ((uint64_t)clock() << 21);
    }
#elif defined(ZAP_PLATFORM_WINDOWS)
    if (BCryptGenRandom(NULL, (PUCHAR)&s, (ULONG)sizeof s,
                        BCRYPT_USE_SYSTEM_PREFERRED_RNG) != 0) {
        s = (uint64_t)time(NULL) ^ ((uint64_t)clock() << 21);
    }
#else
    s = (uint64_t)time(NULL) ^ ((uint64_t)clock() << 21);
#endif
    return s;
}

__attribute__((constructor)) static void zap_hash_seed_init(void) {
    const char *env = getenv("ZAPHASHSEED");
    if (env != NULL && env[0] != '\0' && strcmp(env, "random") != 0) {
        /* A fixed seed for reproducible runs (PYTHONHASHSEED-style, #1064). */
        zap_hash_seed = (uint64_t)strtoull(env, NULL, 10);
    } else {
        zap_hash_seed = zap_hash_os_seed();
    }
}

static uint64_t zap_hash_i64(int64_t k) {
    /* splitmix64 finalizer — cheap, good dispersion. */
    uint64_t x = (uint64_t)k + 0x9e3779b97f4a7c15ULL;
    x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ULL;
    x = (x ^ (x >> 27)) * 0x94d049bb133111ebULL;
    return x ^ (x >> 31);
}

static uint64_t zap_hash_str(const ZapStr *s) {
    /* wyhash, seeded per process (#1064). Collision quality matches the former
     * byte-at-a-time FNV-1a; throughput is far higher on the 4-32B identifier /
     * JSON-field keys dicts see most. The in-table cached-hash design (#837) and
     * the KEY_STR compare path are unchanged — only the hash function changed. */
    return zap_wyhash(s->data, (size_t)s->len, zap_hash_seed);
}

/* `noinline` (#841, D-0149): keeping the key hash out of line keeps the dict
 * fast path (zap_dict_get) tiny — otherwise the string-key FNV loop folds into
 * the inlinable fast path and puffs the typed wrapper back over LLVM's inline
 * threshold, so the LTO cross-module inliner declines to pull it into a hot loop
 * (observed: the wrapper stayed ~220 bytes and out of line). The hash is needed
 * on every probe regardless, so an out-of-line call here costs little, and the
 * set/resize callers are unaffected. */
__attribute__((noinline)) static uint64_t zap_key_hash(int64_t key, int keykind,
                                                       void *hashfn) {
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
    /* Mix one field hash into the accumulator with wyhash's MUM primitive
     * (#1064), for consistency with the str/key hash and stronger avalanche than
     * the former FNV-multiply + splitmix64. Field order matters (non-commutative)
     * and small values spread out. Seedless and deterministic: the per-process
     * seed enters a derived/hand-written struct hash only through its str fields'
     * hashes, so two structs equal within a run hash equal (a == b ⟹
     * hash(a) == hash(b), D-0047). The xor constant keeps combine(0, 0) off 0. */
    return (int64_t)zap_wymix((uint64_t)acc ^ 0x9e3779b97f4a7c15ULL,
                              (uint64_t)h);
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

/* ---------------------------------------------------------------------------
 * Stable list sort (ticket #336, design/RUNTIME-ABI.md §"list.sort / sorted").
 * Defined here, after `zap_str_cmp`, so the str comparator is in scope (the heap
 * section below relies on the same ordering).
 *
 * `zap_list_sort_by` sorts `dst` in place using a parallel `keys` list of the
 * same length: element `dst[i]` is ordered by key `keys[i]`. The no-key cases
 * (`xs.sort()` / `sorted(xs)`) pass `keys == dst`, so elements are compared by
 * their own word; a `key=` projection passes a separately-built key list (the
 * compiler builds it in compiled code, where the growing list is a GC root — a
 * C loop calling back into Zap could not hold it safely across a safepoint).
 *
 * The sort is a stable bottom-up merge over an index permutation: the key array
 * is fully read to order the indices *before* any element is moved, so aliasing
 * `keys == dst` is safe. It makes NO call back into compiled Zap and allocates
 * only non-GC scratch, so no safepoint runs inside it and every element/key
 * word stays rooted (through `dst`/`keys`) for the whole sort.
 *
 * `cmp_kind` selects how two key words compare: ZAP_SORT_I64 is a signed 64-bit
 * compare (int/bool), ZAP_SORT_STR a byte-wise `ZapStr` compare. `reverse` flips
 * the order while preserving stability (equal keys keep their input order, as in
 * Python's `reverse=True`). ------------------------------------------------- */
enum { ZAP_SORT_I64 = 0, ZAP_SORT_STR = 1 };

/* Three-way compare of two key words under `cmp_kind`. */
static int64_t zap_sort_cmp(int64_t a, int64_t b, int64_t cmp_kind) {
    if (cmp_kind == ZAP_SORT_STR) {
        return zap_str_cmp((void *)(intptr_t)a, (void *)(intptr_t)b);
    }
    return (a < b) ? -1 : (a > b ? 1 : 0);
}

/* Stable merge sort of the index array `idx[0..n)` by `keys[idx[.]]`. `tmp` is
 * caller-provided scratch of `n` words. Equal keys keep their relative order
 * (take from the left run on a tie), which holds under `reverse` too since the
 * tie branch is independent of the sign flip. */
static void zap_sort_indices(int64_t *idx, int64_t *tmp, int64_t n,
                             const int64_t *keys, int64_t cmp_kind, int64_t reverse) {
    for (int64_t width = 1; width < n; width *= 2) {
        for (int64_t lo = 0; lo < n; lo += 2 * width) {
            int64_t mid = lo + width < n ? lo + width : n;
            int64_t hi = lo + 2 * width < n ? lo + 2 * width : n;
            int64_t i = lo, j = mid, k = lo;
            while (i < mid && j < hi) {
                int64_t c = zap_sort_cmp(keys[idx[i]], keys[idx[j]], cmp_kind);
                if (reverse) {
                    c = -c;
                }
                if (c <= 0) {
                    tmp[k++] = idx[i++]; /* <= keeps the left (earlier) run first */
                } else {
                    tmp[k++] = idx[j++];
                }
            }
            while (i < mid) {
                tmp[k++] = idx[i++];
            }
            while (j < hi) {
                tmp[k++] = idx[j++];
            }
        }
        memcpy(idx, tmp, (size_t)n * sizeof(int64_t));
    }
}

void zap_list_sort_by(void *dstp, void *keysp, int64_t cmp_kind, int64_t reverse) {
    ZapList *dst = (ZapList *)dstp;
    ZapList *keys = (ZapList *)keysp;
    int64_t n = dst->len;
    if (n < 2) {
        return;
    }
    /* All scratch is plain C memory, freed before returning: this routine makes
     * no allocation that can trigger a collection, so raw buffers are safe. */
    int64_t *idx = (int64_t *)malloc((size_t)n * sizeof(int64_t));
    int64_t *tmp = (int64_t *)malloc((size_t)n * sizeof(int64_t));
    int64_t *out = (int64_t *)malloc((size_t)n * sizeof(int64_t));
    if (!idx || !tmp || !out) {
        fprintf(stderr, "zap: out of memory sorting a list of %lld\n", (long long)n);
        abort();
    }
    for (int64_t i = 0; i < n; i++) {
        idx[i] = i;
    }
    zap_sort_indices(idx, tmp, n, keys->data, cmp_kind, reverse);
    /* Gather the permuted elements, then write them back in one pass — reading
     * every element before overwriting keeps a `keys == dst` alias correct. */
    for (int64_t i = 0; i < n; i++) {
        out[i] = dst->data[idx[i]];
    }
    memcpy(dst->data, out, (size_t)n * sizeof(int64_t));
    free(idx);
    free(tmp);
    free(out);
}

/* Rehash from each entry's cached hash (#837) — no keykind needed, since the
 * stored hash word is already correct regardless of key kind. */
static void zap_dict_resize(ZapDict *d) {
    int64_t ncap = d->cap < 8 ? 8 : d->cap * 2;
    void *nbuf = zap_ptrbuf(zap_d_bytes(ncap));
    uint8_t *ntags = (uint8_t *)nbuf;
    int64_t *nkeys = (int64_t *)((char *)nbuf + ncap);
    int64_t *nvals = (int64_t *)((char *)nbuf + ncap * 9);
    uint64_t *nhashes = (uint64_t *)((char *)nbuf + ncap * 17);
    memset(ntags, ZAP_TAG_EMPTY, (size_t)ncap);
    if (d->cap) {
        /* `zap_ptrbuf` never collects mid-call and resize makes no other call, so
         * the old and new raw buffers stay put for the whole rehash. */
        uint8_t *otags = zap_d_tags(d);
        int64_t *okeys = zap_d_keys(d);
        int64_t *ovals = zap_d_vals(d);
        uint64_t *ohashes = zap_d_hashes(d);
        for (int64_t i = 0; i < d->cap; i++) {
            if (otags[i] < 2) { /* ZAP_TAG_EMPTY or ZAP_TAG_TOMB: drop (reclaims tombstones) */
                continue;
            }
            /* Rehash from the cached hash (#837): the key bytes are unchanged, so
             * the stored hash word is still correct — no FNV loop / __hash__ call,
             * and tombstones are not carried over. */
            uint64_t h = ohashes[i];
            int64_t j = (int64_t)(h & (uint64_t)(ncap - 1));
            while (ntags[j] != ZAP_TAG_EMPTY) {
                j = (j + 1) & (ncap - 1);
            }
            ntags[j] = otags[i];
            nkeys[j] = okeys[i];
            nvals[j] = ovals[i];
            nhashes[j] = h;
        }
    }
    d->buf = nbuf;
    d->cap = ncap;
}

static void zap_dict_set(void *dp, int64_t key, int keykind, int64_t val) {
    ZapDict *d = (ZapDict *)dp;
    if (d->cap == 0 || (d->len + 1) * 4 >= d->cap * 3) { /* load factor 0.75 */
        zap_dict_resize(d);
    }
    /* Compute the hash (KEY_FN runs the user `__hash__`, which may relocate the
     * buffer) BEFORE taking the region bases. */
    uint64_t h = zap_key_hash(key, keykind, d->hashfn);
    uint8_t t = zap_tag(h);
    int64_t j = (int64_t)(h & (uint64_t)(d->cap - 1));
    int64_t tomb = -1;
    uint8_t *tags = zap_d_tags(d);
    while (tags[j] != ZAP_TAG_EMPTY) {
        /* Reject on the 1-byte tag first; only a tag hit touches a key/hash. An
         * int key compares its word directly (no hash gate needed); a str/struct
         * key gates the (expensive) memcmp / __eq__ on the full cached hash (#837,
         * #840). Re-derive the bases each use so a KEY_FN compare that relocated
         * the buffer is picked up (the int/str path has no such call). */
        if (tags[j] == t) {
            if (keykind == KEY_I64) {
                if (zap_d_keys(d)[j] == key) {
                    zap_d_vals(d)[j] = val; /* overwrite existing key */
                    return;
                }
            } else if (zap_d_hashes(d)[j] == h &&
                       zap_key_eq(zap_d_keys(d)[j], key, keykind, d->eqfn)) {
                zap_d_vals(d)[j] = val; /* overwrite existing key */
                return;
            }
            tags = zap_d_tags(d); /* a KEY_FN __eq__ may have moved the buffer */
        } else if (tags[j] == ZAP_TAG_TOMB && tomb < 0) {
            tomb = j;
        }
        j = (j + 1) & (d->cap - 1);
    }
    if (tomb >= 0) {
        j = tomb;
    }
    zap_d_tags(d)[j] = t;
    zap_d_keys(d)[j] = key;
    zap_d_vals(d)[j] = val;
    zap_d_hashes(d)[j] = h;
    d->len++;
}

/* Missing-key get returns 0 (null word) for this increment; membership and a
 * real KeyError wait for the exception runtime (ticket #22). */
/* Dict get is split into a small inlinable FAST path and an out-of-line SLOW
 * path (#841, D-0149). The fast path handles the common case — the key is found
 * at the FIRST probe by word/pointer IDENTITY — in a few instructions, and
 * tail-calls the slow path for everything else (value comparison, probing,
 * tombstones, misses, and user-struct keys). `always_inline` lets the fast path
 * fold into the typed `zap_dict_get_*` wrappers, so under the LTO build LLVM's
 * cross-module inliner pulls it into the generated hot loop; the slow path is
 * `noinline` so it never bloats that inlinable fast path (a monolithic get is
 * far over the inline threshold and would be left out of line, defeating the
 * point — the measurement in #841). This also helps the ordinary object-link
 * build: the wrapper now carries the fast check inline. */
__attribute__((noinline)) static int64_t zap_dict_get_slow(void *dp, int64_t key,
                                                            int keykind, uint64_t h) {
    ZapDict *d = (ZapDict *)dp;
    uint8_t t = zap_tag(h);
    int64_t j = (int64_t)(h & (uint64_t)(d->cap - 1));
    uint8_t *tags = zap_d_tags(d);
    while (tags[j] != ZAP_TAG_EMPTY) {
        /* 1-byte tag reject; a tag hit compares an int key word directly, or
         * gates a str/struct key's compare on the full cached hash (#837, #840). */
        if (tags[j] == t) {
            if (keykind == KEY_I64) {
                if (zap_d_keys(d)[j] == key) {
                    return zap_d_vals(d)[j];
                }
            } else if (zap_d_hashes(d)[j] == h &&
                       zap_key_eq(zap_d_keys(d)[j], key, keykind, d->eqfn)) {
                return zap_d_vals(d)[j];
            }
            tags = zap_d_tags(d); /* a KEY_FN __eq__ may have moved the buffer */
        }
        j = (j + 1) & (d->cap - 1);
    }
    return 0;
}

__attribute__((always_inline)) static inline int64_t zap_dict_get(void *dp, int64_t key,
                                                                  int keykind) {
    ZapDict *d = (ZapDict *)dp;
    if (d->cap == 0) {
        return 0;
    }
    uint64_t h = zap_key_hash(key, keykind, d->hashfn);
    uint8_t t = zap_tag(h);
    int64_t j = (int64_t)(h & (uint64_t)(d->cap - 1));
    /* Fast path: first probe, tag hit, key found by IDENTITY. For int keys the
     * key word IS the value, so identity == equality (and no hash gate is
     * needed). For string keys, identical ZapStr pointers (interned literals,
     * D-0145, or any same-object key) are value-equal, so a repeated `d["k"]`
     * short-circuits with no byte compare. Identity is only ever a SUFFICIENT
     * condition here; anything it does not settle — a different object needing a
     * real compare, a probe past the first slot, a tombstone, or a miss — falls
     * through to the slow path's full `zap_key_eq`. User-struct keys (KEY_FN)
     * always take the slow path so a custom `__eq__` is always consulted. */
    if (keykind != KEY_FN && zap_d_tags(d)[j] == t && zap_d_keys(d)[j] == key) {
        return zap_d_vals(d)[j];
    }
    return zap_dict_get_slow(dp, key, keykind, h);
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

/* `dict[K, float64]` — a float VALUE (#800). The value shares the uniform 8-byte
 * word via a bit reinterpret (like `list[float]`, I#569); it is not a GC pointer,
 * so `val_ptr` stays 0 and the collector never follows it. Hashing/equality are
 * unaffected — only the *key* is hashed, and a float key is a separate follow-up.
 * One setter/getter per key kind (`i64`/`str`/`fn`); a missing key reads back as
 * `0.0` (the null word reinterpreted), matching the non-raising `i64` read. */
void zap_dict_set_i64_f64(void *d, int64_t k, double v) {
    zap_dict_set(d, k, KEY_I64, zap_f64_bits(v));
}
void zap_dict_set_str_f64(void *d, void *k, double v) {
    ((ZapDict *)d)->key_ptr = 1;
    zap_dict_set(d, (int64_t)(intptr_t)k, KEY_STR, zap_f64_bits(v));
}
void zap_dict_set_fn_f64(void *dp, void *k, double v, void *hashfn, void *eqfn) {
    ZapDict *d = (ZapDict *)dp;
    d->key_ptr = 1;
    d->hashfn = hashfn;
    d->eqfn = eqfn;
    zap_dict_set(dp, (int64_t)(intptr_t)k, KEY_FN, zap_f64_bits(v));
}
double zap_dict_get_i64_f64(void *d, int64_t k) {
    return zap_f64_from_bits(zap_dict_get(d, k, KEY_I64));
}
double zap_dict_get_str_f64(void *d, void *k) {
    return zap_f64_from_bits(zap_dict_get(d, (int64_t)(intptr_t)k, KEY_STR));
}

/* ---- Borrowed-bytes dict probes (#1082) ---------------------------------- */
/* Probe a str-keyed dict with a non-owning view `(backing, byte_off, byte_len)`
 * instead of an owned `ZapStr` key: NO transient key allocation and NO root.
 * The hash (seeded wyhash, #1064) and the per-slot compare operate on the
 * borrowed bytes directly, so the result is byte-for-byte identical to the
 * `zap_dict_get_str_*` / `zap_dict_contains_str` path with the materialized key.
 * Returns the matching slot index, or -1. */
static int64_t zap_dict_sv_slot(ZapDict *d, const char *data, int64_t len) {
    if (d->cap == 0) {
        return -1;
    }
    uint64_t h = zap_wyhash(data, (size_t)len, zap_hash_seed);
    uint8_t t = zap_tag(h);
    int64_t j = (int64_t)(h & (uint64_t)(d->cap - 1));
    uint8_t *tags = zap_d_tags(d);
    while (tags[j] != ZAP_TAG_EMPTY) {
        /* 1-byte tag reject, then gate the byte compare on the full cached hash
         * (#837), exactly like the owned str-key slow path. */
        if (tags[j] == t && zap_d_hashes(d)[j] == h) {
            const ZapStr *k = (const ZapStr *)(intptr_t)zap_d_keys(d)[j];
            if (k->len == len && memcmp(k->data, data, (size_t)len) == 0) {
                return j;
            }
        }
        j = (j + 1) & (d->cap - 1);
    }
    return -1;
}
static int64_t zap_dict_sv_word(void *dp, void *bp, int64_t off, int64_t len) {
    ZapDict *d = (ZapDict *)dp;
    int64_t j = zap_dict_sv_slot(d, ((const ZapStr *)bp)->data + off, len);
    return j < 0 ? 0 : zap_d_vals(d)[j];
}
int64_t zap_dict_get_strview_i64(void *dp, void *bp, int64_t off, int64_t len) {
    return zap_dict_sv_word(dp, bp, off, len);
}
void *zap_dict_get_strview_ptr(void *dp, void *bp, int64_t off, int64_t len) {
    return (void *)(intptr_t)zap_dict_sv_word(dp, bp, off, len);
}
double zap_dict_get_strview_f64(void *dp, void *bp, int64_t off, int64_t len) {
    return zap_f64_from_bits(zap_dict_sv_word(dp, bp, off, len));
}
int64_t zap_dict_contains_strview(void *dp, void *bp, int64_t off, int64_t len) {
    ZapDict *d = (ZapDict *)dp;
    return zap_dict_sv_slot(d, ((const ZapStr *)bp)->data + off, len) >= 0 ? 1 : 0;
}
double zap_dict_get_fn_f64(void *dp, void *k, void *hashfn, void *eqfn) {
    ZapDict *d = (ZapDict *)dp;
    d->hashfn = hashfn;
    d->eqfn = eqfn;
    return zap_f64_from_bits(zap_dict_get(dp, (int64_t)(intptr_t)k, KEY_FN));
}

/* `dict[float64, V]` — a float KEY (#803, D-0138). The key word is the canonical
 * IEEE bits of `k` (`-0.0 -> +0.0`, NaN rejected with a catchable ValueError) via
 * `zap_f64_key_word`, then stored/hashed/compared as a plain i64 word (KEY_I64) —
 * so `0.0` and `-0.0` collide and float equality is bit equality. The key is not
 * a GC pointer, so `key_ptr` stays 0. One setter/getter per value kind
 * (i64 / ptr / f64); a missing key reads back as the null word (0 / NULL / 0.0),
 * matching the non-raising `i64`/`str`-keyed read. */
#define ZAP_DICT_F64_KEY(k) \
    zap_f64_key_word((k), ZAP_F64_DICT_KEY_NAN, (int64_t)sizeof(ZAP_F64_DICT_KEY_NAN) - 1)
void zap_dict_set_f64_i64(void *d, double k, int64_t v) {
    zap_dict_set(d, ZAP_DICT_F64_KEY(k), KEY_I64, v);
}
void zap_dict_set_f64_ptr(void *d, double k, void *v) {
    ((ZapDict *)d)->val_ptr = 1;
    zap_dict_set(d, ZAP_DICT_F64_KEY(k), KEY_I64, (int64_t)(intptr_t)v);
}
void zap_dict_set_f64_f64(void *d, double k, double v) {
    zap_dict_set(d, ZAP_DICT_F64_KEY(k), KEY_I64, zap_f64_bits(v));
}
int64_t zap_dict_get_f64_i64(void *d, double k) {
    return zap_dict_get(d, ZAP_DICT_F64_KEY(k), KEY_I64);
}
void *zap_dict_get_f64_ptr(void *d, double k) {
    return (void *)(intptr_t)zap_dict_get(d, ZAP_DICT_F64_KEY(k), KEY_I64);
}
double zap_dict_get_f64_f64(void *d, double k) {
    return zap_f64_from_bits(zap_dict_get(d, ZAP_DICT_F64_KEY(k), KEY_I64));
}

/* `del d[k]` (#595): remove the entry for `key` by tombstoning its slot, reusing
 * the same open-addressing probe as `zap_dict_set`/`zap_dict_get`. A ZAP_TAG_TOMB
 * tag keeps later keys on the same probe chain reachable; the slot is reclaimed on
 * the next resize. A missing key is a silent no-op, matching the current non-
 * raising dict read `d[k]` (a real `KeyError` is tracked under #22). The vacated
 * key/value words are cleared so the collector does not pin the removed
 * referents. */
static void zap_dict_del(void *dp, int64_t key, int keykind) {
    ZapDict *d = (ZapDict *)dp;
    if (d->cap == 0) {
        return;
    }
    uint64_t h = zap_key_hash(key, keykind, d->hashfn);
    uint8_t t = zap_tag(h);
    int64_t j = (int64_t)(h & (uint64_t)(d->cap - 1));
    uint8_t *tags = zap_d_tags(d);
    while (tags[j] != ZAP_TAG_EMPTY) {
        if (tags[j] == t &&
            (keykind == KEY_I64
                 ? (zap_d_keys(d)[j] == key)
                 : (zap_d_hashes(d)[j] == h &&
                    zap_key_eq(zap_d_keys(d)[j], key, keykind, d->eqfn)))) {
            zap_d_tags(d)[j] = ZAP_TAG_TOMB; /* keeps the probe chain reachable */
            zap_d_keys(d)[j] = 0;            /* clear referents so GC can reclaim */
            zap_d_vals(d)[j] = 0;
            d->len--;
            return;
        }
        if (tags[j] == t) {
            tags = zap_d_tags(d); /* a KEY_FN __eq__ may have moved the buffer */
        }
        j = (j + 1) & (d->cap - 1);
    }
}

void zap_dict_del_i64(void *d, int64_t k) { zap_dict_del(d, k, KEY_I64); }
void zap_dict_del_str(void *d, void *k) {
    zap_dict_del(d, (int64_t)(intptr_t)k, KEY_STR);
}
void zap_dict_del_fn(void *dp, void *k, void *hashfn, void *eqfn) {
    ZapDict *d = (ZapDict *)dp;
    d->hashfn = hashfn;
    d->eqfn = eqfn;
    zap_dict_del(dp, (int64_t)(intptr_t)k, KEY_FN);
}
/* `del d[k]` for a float-keyed dict (#803): canonicalize + NaN-reject the key,
 * then tombstone its i64-word slot. A missing key is a no-op (like the other key
 * kinds); a NaN key raises before the probe. */
void zap_dict_del_f64(void *d, double k) {
    zap_dict_del(d, ZAP_DICT_F64_KEY(k), KEY_I64);
}

/* --- dict enumeration / membership / copy (ticket #299) ---------------------
 * A dict is an open-addressing hash table with no insertion-order tracking, so
 * enumeration order is unspecified (hash/probe order) — the same guarantee the
 * json object-key walker documents (D-0059). The key/value word kind (int64 vs
 * pointer) is known statically by codegen, which picks the matching entry
 * point; keys/values are collected with the typed list push so the snapshot
 * list's element tag is set and precise GC still traces pointer elements even
 * if the source dict later dies. */

/* Presence probe (distinct from `zap_dict_get`, whose 0 is ambiguous with a
 * stored 0 word): report whether `key` has a live slot. */
static int zap_dict_has(void *dp, int64_t key, int keykind) {
    ZapDict *d = (ZapDict *)dp;
    if (d->cap == 0) {
        return 0;
    }
    uint64_t h = zap_key_hash(key, keykind, d->hashfn);
    uint8_t t = zap_tag(h);
    int64_t j = (int64_t)(h & (uint64_t)(d->cap - 1));
    uint8_t *tags = zap_d_tags(d);
    while (tags[j] != ZAP_TAG_EMPTY) {
        if (tags[j] == t) {
            if (keykind == KEY_I64) {
                if (zap_d_keys(d)[j] == key) {
                    return 1;
                }
            } else if (zap_d_hashes(d)[j] == h &&
                       zap_key_eq(zap_d_keys(d)[j], key, keykind, d->eqfn)) {
                return 1;
            }
            tags = zap_d_tags(d); /* a KEY_FN __eq__ may have moved the buffer */
        }
        j = (j + 1) & (d->cap - 1);
    }
    return 0;
}

/* Enumeration snapshots. `zap_list_push_*` allocates, but allocation never
 * relocates the dict buffer (Immix only flags a wanted collection mid-alloc;
 * Boehm is non-moving), so the region bases are cached once. A full slot is
 * `tag >= 2` (ZAP_TAG_EMPTY / ZAP_TAG_TOMB are the two reserved low values). */
void *zap_dict_keys_i64(void *dp) {
    ZapDict *d = (ZapDict *)dp;
    void *out = zap_list_new();
    uint8_t *tags = zap_d_tags(d);
    int64_t *keys = zap_d_keys(d);
    for (int64_t i = 0; i < d->cap; i++) {
        if (tags[i] >= 2) {
            zap_list_push_i64(out, keys[i]);
        }
    }
    return out;
}
void *zap_dict_keys_ptr(void *dp) {
    ZapDict *d = (ZapDict *)dp;
    void *out = zap_list_new();
    uint8_t *tags = zap_d_tags(d);
    int64_t *keys = zap_d_keys(d);
    for (int64_t i = 0; i < d->cap; i++) {
        if (tags[i] >= 2) {
            zap_list_push_ptr(out, (void *)(intptr_t)keys[i]);
        }
    }
    return out;
}
/* `.keys()` / `for k in d` of a `dict[float64, V]` (#803): each key word is the
 * canonical IEEE bits, so reinterpret it as a `double` into a fresh `list[float64]`
 * (typed push sets the `f64` element tag so read-back yields a `double`). */
void *zap_dict_keys_f64(void *dp) {
    ZapDict *d = (ZapDict *)dp;
    void *out = zap_list_new();
    uint8_t *tags = zap_d_tags(d);
    int64_t *keys = zap_d_keys(d);
    for (int64_t i = 0; i < d->cap; i++) {
        if (tags[i] >= 2) {
            zap_list_push_f64(out, zap_f64_from_bits(keys[i]));
        }
    }
    return out;
}
void *zap_dict_values_i64(void *dp) {
    ZapDict *d = (ZapDict *)dp;
    void *out = zap_list_new();
    uint8_t *tags = zap_d_tags(d);
    int64_t *vals = zap_d_vals(d);
    for (int64_t i = 0; i < d->cap; i++) {
        if (tags[i] >= 2) {
            zap_list_push_i64(out, vals[i]);
        }
    }
    return out;
}
void *zap_dict_values_ptr(void *dp) {
    ZapDict *d = (ZapDict *)dp;
    void *out = zap_list_new();
    uint8_t *tags = zap_d_tags(d);
    int64_t *vals = zap_d_vals(d);
    for (int64_t i = 0; i < d->cap; i++) {
        if (tags[i] >= 2) {
            zap_list_push_ptr(out, (void *)(intptr_t)vals[i]);
        }
    }
    return out;
}
/* `.values()` of a `dict[K, float64]` (#800): snapshot the float value words into
 * a fresh `list[float64]` via the typed float push, so the list's element tag is
 * `f64` and read-back reinterprets each word as a `double`. */
void *zap_dict_values_f64(void *dp) {
    ZapDict *d = (ZapDict *)dp;
    void *out = zap_list_new();
    uint8_t *tags = zap_d_tags(d);
    int64_t *vals = zap_d_vals(d);
    for (int64_t i = 0; i < d->cap; i++) {
        if (tags[i] >= 2) {
            zap_list_push_f64(out, zap_f64_from_bits(vals[i]));
        }
    }
    return out;
}
int64_t zap_dict_contains_i64(void *d, int64_t k) {
    return zap_dict_has(d, k, KEY_I64);
}
int64_t zap_dict_contains_str(void *d, void *k) {
    return zap_dict_has(d, (int64_t)(intptr_t)k, KEY_STR);
}
int64_t zap_dict_contains_fn(void *dp, void *k, void *hashfn, void *eqfn) {
    ZapDict *d = (ZapDict *)dp;
    d->hashfn = hashfn;
    d->eqfn = eqfn;
    return zap_dict_has(dp, (int64_t)(intptr_t)k, KEY_FN);
}
/* `k in d` for a float-keyed dict (#803): canonicalize + NaN-reject, then probe
 * the i64-word slot. A NaN key raises (catchable) rather than reporting absence. */
int64_t zap_dict_contains_f64(void *d, double k) {
    return zap_dict_has(d, ZAP_DICT_F64_KEY(k), KEY_I64);
}

/* Shallow copy: a fresh dict with the same entries. Key and value words are
 * copied verbatim, so referenced objects (str/struct/nested collections) are
 * shared, not cloned — letting a caller extend a map without mutating the
 * original (e.g. default headers + per-call overrides). */
void *zap_dict_copy(void *sp) {
    ZapDict *s = (ZapDict *)sp;
    ZapDict *d = (ZapDict *)zap_dict_new();
    d->key_ptr = s->key_ptr;
    d->val_ptr = s->val_ptr;
    d->hashfn = s->hashfn;
    d->eqfn = s->eqfn;
    if (s->cap == 0) {
        return d;
    }
    /* One contiguous buffer: copy all four regions verbatim in a single memcpy
     * (slot layout and slot indices are preserved, so no rehash is needed). No
     * allocation/callback runs between the alloc and the copy, so both raw
     * buffers stay put. */
    void *nbuf = zap_ptrbuf(zap_d_bytes(s->cap));
    memcpy(nbuf, s->buf, (size_t)zap_d_bytes(s->cap));
    d->buf = nbuf;
    d->cap = s->cap;
    d->len = s->len;
    return d;
}

/* set[T]: a dict without values — open-addressing hash table of element words.
 * Shares the key hashing/equality above (RUNTIME-ABI: "dict and set can share
 * hashing"). Elements are uniform 8-byte words, like dict keys; the element
 * kind (int64 vs str) is passed per call and is stable for a given set.
 *
 * Same SoA table as the dict (#840, D-0156), minus the values region: one buffer
 * carved into tags[cap] (1B) | keys[cap] (8B) | hashes[cap] (8B) = 17*cap bytes.
 * A probe scans the dense tag stream; a tag hit compares an int element directly
 * or gates the str/struct compare on the cached hash (which also lets resize
 * rehash without an FNV/__hash__ recompute — a small gain over the pre-#840 set,
 * which had no cached hash). */
typedef struct {
    int64_t len;
    int64_t cap; /* number of slots; always a power of two or 0 */
    void *buf;   /* tags[cap] | keys[cap] | hashes[cap]; NULL while cap == 0 */
    void *hashfn; /* struct elements only (KEY_FN): the type's __hash__/__eq__ */
    void *eqfn;
    uint8_t elem_ptr; /* elements are GC pointers (str/struct) */
} ZapSet;

static inline uint8_t *zap_s_tags(const ZapSet *s) { return (uint8_t *)s->buf; }
static inline int64_t *zap_s_keys(const ZapSet *s) {
    return (int64_t *)((char *)s->buf + s->cap);
}
static inline uint64_t *zap_s_hashes(const ZapSet *s) {
    return (uint64_t *)((char *)s->buf + s->cap * 9);
}
static inline int64_t zap_s_bytes(int64_t cap) { return cap * 17; }

void *zap_set_new(void) {
    ZapSet *s = (ZapSet *)zap_obj(&zap_desc_set);
    s->len = 0;
    s->cap = 0;
    s->buf = NULL;
    s->hashfn = NULL;
    s->eqfn = NULL;
    return s;
}

static void zap_set_resize(ZapSet *s) {
    int64_t ncap = s->cap < 8 ? 8 : s->cap * 2;
    void *nbuf = zap_ptrbuf(zap_s_bytes(ncap));
    uint8_t *ntags = (uint8_t *)nbuf;
    int64_t *nkeys = (int64_t *)((char *)nbuf + ncap);
    uint64_t *nhashes = (uint64_t *)((char *)nbuf + ncap * 9);
    memset(ntags, ZAP_TAG_EMPTY, (size_t)ncap);
    if (s->cap) {
        uint8_t *otags = zap_s_tags(s);
        int64_t *okeys = zap_s_keys(s);
        uint64_t *ohashes = zap_s_hashes(s);
        for (int64_t i = 0; i < s->cap; i++) {
            if (otags[i] < 2) {
                continue; /* empty or tomb (set has no delete today, but be safe) */
            }
            uint64_t h = ohashes[i];
            int64_t j = (int64_t)(h & (uint64_t)(ncap - 1));
            while (ntags[j] != ZAP_TAG_EMPTY) {
                j = (j + 1) & (ncap - 1);
            }
            ntags[j] = otags[i];
            nkeys[j] = okeys[i];
            nhashes[j] = h;
        }
    }
    s->buf = nbuf;
    s->cap = ncap;
}

static void zap_set_add(void *sp, int64_t key, int keykind) {
    ZapSet *s = (ZapSet *)sp;
    if (s->cap == 0 || (s->len + 1) * 4 >= s->cap * 3) { /* load factor 0.75 */
        zap_set_resize(s);
    }
    uint64_t h = zap_key_hash(key, keykind, s->hashfn);
    uint8_t t = zap_tag(h);
    int64_t j = (int64_t)(h & (uint64_t)(s->cap - 1));
    uint8_t *tags = zap_s_tags(s);
    while (tags[j] != ZAP_TAG_EMPTY) {
        if (tags[j] == t) {
            if (keykind == KEY_I64) {
                if (zap_s_keys(s)[j] == key) {
                    return; /* already present */
                }
            } else if (zap_s_hashes(s)[j] == h &&
                       zap_key_eq(zap_s_keys(s)[j], key, keykind, s->eqfn)) {
                return; /* already present */
            }
            tags = zap_s_tags(s); /* a KEY_FN __eq__ may have moved the buffer */
        }
        j = (j + 1) & (s->cap - 1);
    }
    zap_s_tags(s)[j] = t;
    zap_s_keys(s)[j] = key;
    zap_s_hashes(s)[j] = h;
    s->len++;
}

static int64_t zap_set_contains(void *sp, int64_t key, int keykind) {
    ZapSet *s = (ZapSet *)sp;
    if (s->cap == 0) {
        return 0;
    }
    uint64_t h = zap_key_hash(key, keykind, s->hashfn);
    uint8_t t = zap_tag(h);
    int64_t j = (int64_t)(h & (uint64_t)(s->cap - 1));
    uint8_t *tags = zap_s_tags(s);
    while (tags[j] != ZAP_TAG_EMPTY) {
        if (tags[j] == t) {
            if (keykind == KEY_I64) {
                if (zap_s_keys(s)[j] == key) {
                    return 1;
                }
            } else if (zap_s_hashes(s)[j] == h &&
                       zap_key_eq(zap_s_keys(s)[j], key, keykind, s->eqfn)) {
                return 1;
            }
            tags = zap_s_tags(s); /* a KEY_FN __eq__ may have moved the buffer */
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
/* `set[float64]` — a float MEMBER (#803, D-0138). The member word is the
 * canonical IEEE bits of `v` (`-0.0 -> +0.0`, NaN rejected with a catchable
 * ValueError) via `zap_f64_key_word`, then stored/hashed/compared as a plain i64
 * word (KEY_I64), so `0.0` and `-0.0` collapse to one member. A member is not a
 * GC pointer, so `elem_ptr` stays 0 and `zap_set_items` snapshots the float words
 * verbatim (read back as `double` via the `f64` list element tag at the iteration
 * site). `set` has no element-removal entry point today, so the NaN-on-delete
 * rule is carried by the dict `del` path (D-0138). */
#define ZAP_SET_F64_MEM(v) \
    zap_f64_key_word((v), ZAP_F64_SET_MEM_NAN, (int64_t)sizeof(ZAP_F64_SET_MEM_NAN) - 1)
void zap_set_add_f64(void *s, double v) {
    zap_set_add(s, ZAP_SET_F64_MEM(v), KEY_I64);
}
int64_t zap_set_contains_f64(void *s, double v) {
    return zap_set_contains(s, ZAP_SET_F64_MEM(v), KEY_I64);
}

/* Snapshot the live elements into a fresh list (element words copied verbatim),
 * so `for x in s` can reuse the list iterator. Iteration order is unspecified. */
void *zap_set_items(void *sp) {
    ZapSet *s = (ZapSet *)sp;
    ZapList *out = (ZapList *)zap_list_new();
    uint8_t *tags = zap_s_tags(s);
    int64_t *keys = zap_s_keys(s);
    for (int64_t i = 0; i < s->cap; i++) {
        if (tags[i] >= 2) { /* full slot (ZAP_TAG_EMPTY / ZAP_TAG_TOMB are < 2) */
            zap_list_push(out, keys[i]);
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
enum { HEAP_ELEM_I64 = 0, HEAP_ELEM_STR = 1, HEAP_ELEM_F64 = 2 };
enum { HEAP_MIN = 0, HEAP_MAX = 1 };

/* Total order over `double` for ordered float collections (#800). IEEE `<`/`>`
 * leaves NaN unordered (every comparison false), which would break a heap's
 * invariant, so NaN is defined to sort as the GREATEST value (after +inf): in a
 * min-heap it drains last, in a max-heap first. `-0.0` and `+0.0` compare equal
 * (as `==` already gives), so they tie. Returns -1 / 0 / +1. */
static int64_t zap_f64_total_cmp(double a, double b) {
    if (a < b) {
        return -1;
    }
    if (a > b) {
        return 1;
    }
    if (a == b) {
        return 0; /* includes -0.0 == +0.0 */
    }
    /* At least one operand is NaN. */
    int an = isnan(a), bn = isnan(b);
    if (an && bn) {
        return 0;
    }
    return an ? 1 : -1; /* NaN is greatest */
}

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
    } else if (h->kind == HEAP_ELEM_F64) {
        /* Float elements are stored as reinterpreted words (#800); order them by
         * IEEE value under the NaN-greatest total order. */
        c = zap_f64_total_cmp(zap_f64_from_bits(a), zap_f64_from_bits(b));
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

/* `heap[float64]` (#800): the element word is a reinterpreted `double`, ordered
 * by `zap_f64_total_cmp` (NaN greatest). Storage and GC are identical to an
 * `i64` heap — only the comparator differs — so these wrap the word entry points
 * around the bit reinterpret. */
void zap_heap_push_f64(void *h, double v) {
    zap_heap_push_word((ZapHeap *)h, zap_f64_bits(v));
}
double zap_heap_pop_f64(void *hp) {
    ZapHeap *h = (ZapHeap *)hp;
    zap_heap_require(h, "pop");
    return zap_f64_from_bits(zap_heap_pop_word(h));
}
double zap_heap_peek_f64(void *hp) {
    ZapHeap *h = (ZapHeap *)hp;
    zap_heap_require(h, "peek");
    return zap_f64_from_bits(h->data[0]);
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
 * heap[T] ergonomics (ticket #978, D-0236): O(n) heapify, in-place
 * `heapreplace`/`heappushpop`, and top-/bottom-K (`nlargest`/`nsmallest`,
 * optionally keyed). These reuse the same binary-heap core as the base object
 * (sift up/down, the `dir`-flipped comparator) — no new ordering model. The
 * keyed top-K is *stable*: among elements whose keys tie, the one that came
 * earlier in the input wins (Python `heapq.nlargest`/`nsmallest` semantics),
 * achieved with a secondary original-index tiebreak that is NOT flipped by the
 * min/max direction. Keys are precomputed by the caller (lowering invokes the
 * `key=` closure per element and passes a parallel key list), so the runtime
 * never calls back into Zap.
 *
 * `n` is clamped to `[0, len]` (a negative or oversize count is not an error,
 * matching Python: `nlargest(-1, xs) == []`, `nlargest(len+5, xs)` is the whole
 * sequence sorted). `heapreplace` requires a non-empty heap (aborts, like
 * `pop`); `heappushpop` is defined on the empty heap (it returns the pushed
 * value). ------------------------------------------------------------------ */

/* O(n) bottom-up heapify: build a heap directly from a list's element words
 * (Floyd's construction), rather than `n` sift-up pushes. `kind`/`dir` are as
 * `zap_heap_new`. The list stores 8-byte element words in exactly the heap's
 * representation (i64 value / `ZapStr` pointer bits / reinterpreted `double`),
 * so the words copy across verbatim; we then sift down every internal node from
 * the last parent back to the root. */
void *zap_heap_from_list(void *lp, int64_t kind, int64_t dir) {
    ZapList *xs = (ZapList *)lp;
    int64_t len = xs->len;
    ZapHeap *h = (ZapHeap *)zap_heap_new(kind, dir);
    if (len == 0) {
        return h;
    }
    while (h->cap < len) {
        zap_heap_grow(h);
    }
    memcpy(h->data, xs->data, (size_t)len * sizeof(int64_t));
    h->len = len;
    for (int64_t i = len / 2 - 1; i >= 0; i--) {
        zap_heap_sift_down(h, i);
    }
    return h;
}

/* heapreplace: pop the current root and push `x` in one step, returning the
 * popped (old) root. Because `x` is written straight into the root slot and
 * sifted down, the returned item may be SMALLER than `x` (that is the point of
 * heapreplace, and what distinguishes it from heappushpop). */
static int64_t zap_heap_replace_word(ZapHeap *h, int64_t x) {
    int64_t old = h->data[0];
    h->data[0] = x;
    zap_heap_sift_down(h, 0);
    return old;
}

/* heappushpop: push `x`, then pop the root — returning the higher-priority of
 * `{x} ∪ heap`. If the heap is empty, or `x` already has priority over the root
 * (so `x` would be the new root), `x` is returned and the heap is UNCHANGED;
 * otherwise `x` displaces the root (which is returned) and sifts down. The
 * strict `cmp(root, x) < 0` test mirrors Python's `heap[0] < item`, so a tie
 * returns `x` and leaves the heap untouched. */
static int64_t zap_heap_pushpop_word(ZapHeap *h, int64_t x) {
    if (h->len == 0) {
        return x;
    }
    if (zap_heap_cmp(h, h->data[0], x) < 0) {
        int64_t old = h->data[0];
        h->data[0] = x;
        zap_heap_sift_down(h, 0);
        return old;
    }
    return x;
}

int64_t zap_heap_replace_i64(void *hp, int64_t x) {
    ZapHeap *h = (ZapHeap *)hp;
    zap_heap_require(h, "replace");
    return zap_heap_replace_word(h, x);
}
void *zap_heap_replace_ptr(void *hp, void *x) {
    ZapHeap *h = (ZapHeap *)hp;
    zap_heap_require(h, "replace");
    return (void *)(intptr_t)zap_heap_replace_word(h, (int64_t)(intptr_t)x);
}
double zap_heap_replace_f64(void *hp, double x) {
    ZapHeap *h = (ZapHeap *)hp;
    zap_heap_require(h, "replace");
    return zap_f64_from_bits(zap_heap_replace_word(h, zap_f64_bits(x)));
}

int64_t zap_heap_pushpop_i64(void *hp, int64_t x) {
    return zap_heap_pushpop_word((ZapHeap *)hp, x);
}
void *zap_heap_pushpop_ptr(void *hp, void *x) {
    return (void *)(intptr_t)zap_heap_pushpop_word((ZapHeap *)hp,
                                                    (int64_t)(intptr_t)x);
}
double zap_heap_pushpop_f64(void *hp, double x) {
    return zap_f64_from_bits(
        zap_heap_pushpop_word((ZapHeap *)hp, zap_f64_bits(x)));
}

/* nlargest/nsmallest WITHOUT a key: heapify the whole input (O(n)) into a
 * heap whose direction selects largest-first (max-heap, `dir == HEAP_MAX`) or
 * smallest-first (min-heap), then pop `n` roots into a fresh list — so the
 * result is already in priority order (descending for nlargest, ascending for
 * nsmallest), matching Python. `n` is clamped to `[0, len]`. */
void *zap_heap_topk(void *lp, int64_t n, int64_t dir, int64_t kind) {
    ZapList *xs = (ZapList *)lp;
    int64_t len = xs->len;
    if (n < 0) {
        n = 0;
    }
    if (n > len) {
        n = len;
    }
    ZapList *out = (ZapList *)zap_list_new();
    if (kind == HEAP_ELEM_STR) {
        out->elem_ptr = 1; /* precise GC: result elements are pointers */
    }
    if (n == 0) {
        return out;
    }
    ZapHeap *h = (ZapHeap *)zap_heap_from_list(lp, kind, dir);
    for (int64_t k = 0; k < n; k++) {
        zap_list_push(out, zap_heap_pop_word(h));
    }
    return out;
}

/* A (key, element, original-index) triple for keyed top-K. Stored in a scanned
 * buffer so pointer-valued keys/elements stay live across any GC during the
 * selection. */
typedef struct {
    int64_t key;  /* key word (i64 / ZapStr bits / double bits) */
    int64_t elem; /* element word, returned verbatim */
    int64_t idx;  /* original position, for a stable tiebreak */
} ZapHeapKV;

/* Priority compare for the keyed heap: by key under (`key_kind`, `dir`), with a
 * secondary earlier-index-wins tiebreak that is deliberately NOT flipped by
 * `dir` — so equal keys always surface in input order (stable) whether the
 * caller asked for largest or smallest. Returns < 0 when `a` outranks `b`. */
static int64_t zap_heap_kv_cmp(const ZapHeapKV *a, const ZapHeapKV *b,
                               int64_t key_kind, int64_t dir) {
    int64_t c;
    if (key_kind == HEAP_ELEM_STR) {
        c = zap_str_cmp((void *)(intptr_t)a->key, (void *)(intptr_t)b->key);
    } else if (key_kind == HEAP_ELEM_F64) {
        c = zap_f64_total_cmp(zap_f64_from_bits(a->key),
                              zap_f64_from_bits(b->key));
    } else {
        c = (a->key < b->key) ? -1 : (a->key > b->key ? 1 : 0);
    }
    if (dir == HEAP_MAX) {
        c = -c;
    }
    if (c != 0) {
        return c;
    }
    return (a->idx < b->idx) ? -1 : (a->idx > b->idx ? 1 : 0);
}

static void zap_heap_kv_sift_down(ZapHeapKV *a, int64_t len, int64_t i,
                                  int64_t key_kind, int64_t dir) {
    for (;;) {
        int64_t l = 2 * i + 1;
        int64_t r = 2 * i + 2;
        int64_t top = i;
        if (l < len && zap_heap_kv_cmp(&a[l], &a[top], key_kind, dir) < 0) {
            top = l;
        }
        if (r < len && zap_heap_kv_cmp(&a[r], &a[top], key_kind, dir) < 0) {
            top = r;
        }
        if (top == i) {
            break;
        }
        ZapHeapKV t = a[i];
        a[i] = a[top];
        a[top] = t;
        i = top;
    }
}

/* nlargest/nsmallest WITH a key: `elems` and `keys` are parallel lists (the
 * caller has already evaluated `key=` on each element). Order the (key, elem,
 * idx) triples by key under `dir`, stable on ties, pop `n` roots, and return a
 * list of the corresponding ELEMENTS (not keys) in priority order. `elem_kind`
 * tags the result list for GC. `n` is clamped to `[0, len]`. */
void *zap_heap_topk_keyed(void *ep, void *kp, int64_t n, int64_t dir,
                          int64_t key_kind, int64_t elem_kind) {
    ZapList *elems = (ZapList *)ep;
    ZapList *keys = (ZapList *)kp;
    int64_t len = elems->len;
    if (n < 0) {
        n = 0;
    }
    if (n > len) {
        n = len;
    }
    ZapList *out = (ZapList *)zap_list_new();
    if (elem_kind == HEAP_ELEM_STR) {
        out->elem_ptr = 1;
    }
    if (n == 0) {
        return out;
    }
    ZapHeapKV *a =
        (ZapHeapKV *)zap_ptrbuf((int64_t)len * (int64_t)sizeof(ZapHeapKV));
    for (int64_t i = 0; i < len; i++) {
        a[i].key = keys->data[i];
        a[i].elem = elems->data[i];
        a[i].idx = i;
    }
    for (int64_t i = len / 2 - 1; i >= 0; i--) {
        zap_heap_kv_sift_down(a, len, i, key_kind, dir);
    }
    int64_t heaplen = len;
    for (int64_t k = 0; k < n; k++) {
        ZapHeapKV top = a[0];
        heaplen--;
        if (heaplen > 0) {
            a[0] = a[heaplen];
            zap_heap_kv_sift_down(a, heaplen, 0, key_kind, dir);
        }
        zap_list_push(out, top.elem);
    }
    return out;
}

/* ---------------------------------------------------------------------------
 * Numeric arrays (ticket #24, design/0002-arrays.md, design/RUNTIME-ABI.md
 * §"Collection layout": `array[T]`).
 *
 * A dense, contiguous, fixed-dtype buffer over the dtypes `f64` and `i64`
 * (8-byte element words) and `uint8`/`bool` (1-byte words; ticket #49, image
 * data and masks). The object carries its dtype and an N-D row-major shape
 * (`ndim` + `dims`, #500): rank-1 arrays set `ndim = 1`, and `array_new_nd`
 * builds a rank-`n` array whose `dims` multiply to the element count. A full
 * multi-axis index `m[i, j]` folds through `zap_array_row` (partial index ->
 * rank-reduced view) down to a rank-1 view and a 1-D get; strided views layer
 * on later without an ABI break.
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
 * Deferred: strided views, broadcasting beyond a scalar, and N-D (axis-0)
 * array slicing. N-D shape + multi-axis indexing are supported (#500);
 * contiguous 1-D views and full-shape checks are supported (I#476). */
/* The full dtype matrix (#503 f32/i32, #890 i8/i16 + u16/u32/u64). Codes 0..=3
 * are the original dtypes (ABI-stable, must match `Dtype::code` in ir_core). */
enum {
    DTYPE_F64 = 0,
    DTYPE_I64 = 1,
    DTYPE_U8 = 2,
    DTYPE_BOOL = 3,
    DTYPE_F32 = 4,
    DTYPE_I32 = 5,
    DTYPE_I8 = 6,
    DTYPE_I16 = 7,
    DTYPE_U16 = 8,
    DTYPE_U32 = 9,
    DTYPE_U64 = 10,
    DTYPE_COMPLEX = 11 /* complex128: two contiguous f64 lanes (re, im), 16 bytes */
};

typedef struct {
    int64_t dtype;      /* one of DTYPE_* above */
    int64_t ndim;       /* static rank >= 1 (#500); 1 for a plain 1-D array */
    int64_t *dims;      /* `ndim` row-major dimensions; dims[0] is len(a) */
    int64_t *strides;   /* per-axis element strides (#502); NULL == contiguous */
    void *data;         /* GC buffer base; `off` elements precede this array's first */
    int64_t off;        /* element offset of this array's first element into `data` */
} ZapArray;
/* `off` is a non-zero element offset only for a slice *view* (D-0030): a view
 * shares its parent's `data` buffer (pointing at the buffer BASE, not into its
 * middle — the precise collector forwards `data` as an object start under
 * evacuation, so an interior pointer would corrupt), carrying its window as
 * `off` (first element) + `dims[0]` (length). All element access adds `off`, and
 * `zap_array_data` returns the window base so the SIMD kernels stay contiguous.
 *
 * `strides` (#502, D-0171) is NULL for every contiguous array — the common case,
 * every constructed array — meaning canonical row-major strides (the element
 * stride of axis k is the product of the dims below it, axis ndim-1 being 1). A
 * NON-NULL `strides` marks a strided *view* (a stepped/reversed/N-D slice): it
 * holds one signed element stride per axis, so element (i0..i_{n-1}) lives at
 * `data[off + Σ i_k·strides[k]]`. Bulk kernels (element-wise ops, reductions)
 * never read strided storage directly — they call `zap_array_contig`, which
 * returns a contiguous array unchanged and compacts a strided view to a fresh
 * contiguous copy first; only scalar get/set, indexing, and the view builders
 * are stride-aware (so a write through a strided view still aliases the parent). */

/* Bytes per stored element, by dtype width (#503/#890). */
static size_t zap_array_elem_size(int64_t dtype) {
    switch (dtype) {
    case DTYPE_U8:
    case DTYPE_BOOL:
    case DTYPE_I8:
        return 1;
    case DTYPE_I16:
    case DTYPE_U16:
        return 2;
    case DTYPE_F32:
    case DTYPE_I32:
    case DTYPE_U32:
        return 4;
    case DTYPE_COMPLEX: /* two f64 lanes (re, im) */
        return 16;
    default: /* f64 / i64 / u64 */
        return 8;
    }
}

/* The total number of elements = product of all `ndim` dimensions (row-major,
 * N-D, #500). For a 1-D array this is `dims[0]`; an empty axis makes it 0. The
 * contiguous buffer always holds exactly this many elements starting at `off`. */
static int64_t zap_array_count_of(const ZapArray *a) {
    int64_t n = 1;
    for (int64_t k = 0; k < a->ndim; k++) {
        n *= a->dims[k];
    }
    return n;
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
    a->strides = NULL; /* contiguous (#502) */
    a->data = data;
    a->off = 0;
    return a;
}

/* N-D construction (#500, D-0164). Allocate a rank-`ndim` array whose buffer
 * holds `total` elements (= product of the dims the caller is about to set).
 * The dims buffer is zeroed; codegen fills each axis with `zap_array_set_dim`
 * and then fills the elements by flat (row-major) index with `zap_array_set_*`.
 * Splitting construction this way keeps every runtime entry point fixed-arity
 * (the shape is static per construction site but has no uniform ABI word). */
void *zap_array_new_nd(int64_t dtype, int64_t ndim, int64_t total) {
    if (ndim < 1) {
        ndim = 1;
    }
    ZapArray *a = (ZapArray *)zap_obj(&zap_desc_array);
    int64_t *dims = (int64_t *)zap_buf((size_t)ndim * sizeof(int64_t));
    for (int64_t k = 0; k < ndim; k++) {
        dims[k] = 0;
    }
    size_t esz = zap_array_elem_size(dtype);
    void *data = zap_buf((size_t)(total > 0 ? total : 0) * esz);
    a->dtype = dtype;
    a->ndim = ndim;
    a->dims = dims;
    a->strides = NULL; /* contiguous (#502); dims are filled by set_dim */
    a->data = data;
    a->off = 0;
    return a;
}

void zap_array_set_dim(void *ap, int64_t axis, int64_t size) {
    ((ZapArray *)ap)->dims[axis] = size;
}

/* Allocate a result array with the SAME shape (ndim + dims) as `src` but a
 * possibly different `out_dtype` — the element-wise kernels' result buffer
 * (#500). The dims buffer is copied (never aliased: a fresh buffer avoids an
 * interior GC pointer, like a slice view's `off`), and the data buffer is sized
 * for the full element count so N-D kernels stay flat over the whole shape. */
void *zap_array_new_like(void *srcp, int64_t out_dtype) {
    ZapArray *src = (ZapArray *)srcp;
    ZapArray *a = (ZapArray *)zap_obj(&zap_desc_array);
    int64_t ndim = src->ndim;
    int64_t *dims = (int64_t *)zap_buf((size_t)ndim * sizeof(int64_t));
    int64_t total = 1;
    for (int64_t k = 0; k < ndim; k++) {
        dims[k] = src->dims[k];
        total *= dims[k];
    }
    size_t esz = zap_array_elem_size(out_dtype);
    void *data = zap_buf((size_t)(total > 0 ? total : 0) * esz);
    a->dtype = out_dtype;
    a->ndim = ndim;
    a->dims = dims;
    a->strides = NULL; /* fresh contiguous result (#502) */
    a->data = data;
    a->off = 0;
    return a;
}

/* `len(a)` is the length of the FIRST axis (NumPy `len`), not the total element
 * count — `dims[0]` for every rank. The total is `zap_array_count`. */
int64_t zap_array_len(void *a) { return ((ZapArray *)a)->dims[0]; }

/* `a.ndim` — the static rank; `a.shape[k]` / offset math reads `a.dims[axis]`;
 * `a.size` — the total element count (product of all dims, #500). */
int64_t zap_array_ndim(void *a) { return ((ZapArray *)a)->ndim; }
int64_t zap_array_count(void *a) { return zap_array_count_of((ZapArray *)a); }
int64_t zap_array_dim(void *ap, int64_t axis) {
    ZapArray *a = (ZapArray *)ap;
    if (axis < 0 || axis >= a->ndim) {
        fprintf(stderr, "zap: array axis %lld out of range (ndim %lld)\n",
                (long long)axis, (long long)a->ndim);
        abort();
    }
    return a->dims[axis];
}

/* A partial index `m[i]` on a rank-n (n >= 2) array -> a rank-(n-1) **view**
 * sharing the parent buffer (#500, D-0030 view semantics). Row-major: axis 0 is
 * the outermost, so row `i` begins `i * stride0` elements in, where `stride0` is
 * the product of the trailing dims. The new header copies `dims[1..n]` into a
 * fresh buffer (never an interior pointer into the parent's dims) and advances
 * `off`; `data` stays at the shared base. A full multi-axis index `m[i, j]` is
 * lowered as a fold of rows down to a rank-1 view followed by a 1-D get, so this
 * is only ever called with n >= 2. Python-style out-of-range aborts (catchable
 * index exceptions are a documented follow-up, matching 1-D `a[i]`). */
/* The effective element stride of `axis` (#502): an explicit per-axis stride for
 * a view, else the canonical row-major stride (product of the dims below it). */
static int64_t zap_array_stride_of(const ZapArray *a, int64_t axis) {
    if (a->strides) {
        return a->strides[axis];
    }
    int64_t s = 1;
    for (int64_t k = axis + 1; k < a->ndim; k++) {
        s *= a->dims[k];
    }
    return s;
}

/* Map a flat logical index into a rank-1 array/view to its physical element
 * offset into `data` (#502). A rank-1 strided view steps by `strides[0]`; every
 * other case (a contiguous array of any rank, including the flat fill during N-D
 * construction) is dense, so the flat index is the offset past `off`. */
static int64_t zap_array_phys(const ZapArray *a, int64_t i) {
    if (a->strides && a->ndim == 1) {
        return a->off + i * a->strides[0];
    }
    return a->off + i;
}

/* Map a row-major flat logical index over the WHOLE N-D shape to its physical
 * element offset into `data`, honoring per-axis strides for a view of any rank
 * (#510). Dense (`strides == NULL`) is the flat index past `off`. Used by
 * in-place advanced-index assignment, which writes into the real storage (a may
 * itself be a strided view) and so cannot compact first. */
static int64_t zap_array_phys_nd(const ZapArray *a, int64_t flat) {
    if (!a->strides) {
        return a->off + flat;
    }
    int64_t off = a->off, rem = flat;
    for (int64_t k = a->ndim - 1; k >= 0; k--) {
        int64_t c = rem % a->dims[k];
        rem /= a->dims[k];
        off += c * a->strides[k];
    }
    return off;
}

/* Compact a possibly-strided array to a contiguous one (#502): returns `a`
 * unchanged when it is already contiguous (`strides == NULL`), else allocates a
 * fresh contiguous array and copies the elements in row-major order by walking
 * the strided layout with an odometer over the dims. Bulk kernels (element-wise
 * ops, reductions) call this at entry so their inner loops stay contiguous and
 * oblivious to views; the copy is read-only, so no aliasing is lost (those
 * operations already produce fresh results). */
void *zap_array_contig(void *ap) {
    ZapArray *a = (ZapArray *)ap;
    if (!a->strides) {
        return a; /* already contiguous */
    }
    int64_t ndim = a->ndim;
    int64_t total = zap_array_count_of(a);
    ZapArray *r = (ZapArray *)zap_obj(&zap_desc_array);
    int64_t *dims = (int64_t *)zap_buf((size_t)ndim * sizeof(int64_t));
    for (int64_t k = 0; k < ndim; k++) {
        dims[k] = a->dims[k];
    }
    size_t esz = zap_array_elem_size(a->dtype);
    char *dst = (char *)zap_buf((size_t)(total > 0 ? total : 0) * esz);
    r->dtype = a->dtype;
    r->ndim = ndim;
    r->dims = dims;
    r->strides = NULL;
    r->data = dst;
    r->off = 0;
    /* Odometer over the N-D index space, gathering each element in row-major
     * order. `coord` counts the dims; `phys` tracks the strided source offset. */
    const char *base = (const char *)a->data + (size_t)a->off * esz;
    int64_t *coord = (int64_t *)zap_buf((size_t)ndim * sizeof(int64_t));
    for (int64_t k = 0; k < ndim; k++) {
        coord[k] = 0;
    }
    for (int64_t i = 0; i < total; i++) {
        int64_t phys = 0;
        for (int64_t k = 0; k < ndim; k++) {
            phys += coord[k] * a->strides[k];
        }
        memcpy(dst + (size_t)i * esz, base + (size_t)phys * esz, esz);
        for (int64_t k = ndim - 1; k >= 0; k--) { /* increment the odometer */
            if (++coord[k] < a->dims[k]) {
                break;
            }
            coord[k] = 0;
        }
    }
    return r;
}

/* Integer-index `axis` of `a`, dropping it: `m[i]` is `index_axis(a, 0, i)`
 * (#500/#502). Shares the parent buffer base (GC-safe) and advances `off` by
 * `i` strides of that axis; the surviving axes keep their effective strides, so
 * a contiguous axis-0 index stays contiguous (NULL strides) while any other case
 * carries explicit strides. Out-of-range aborts, like 1-D `a[i]`. */
void *zap_array_index_axis(void *ap, int64_t axis, int64_t i) {
    ZapArray *a = (ZapArray *)ap;
    int64_t dim = a->dims[axis];
    if (i < 0 || i >= dim) {
        fprintf(stderr, "zap: array index %lld out of range (len %lld)\n",
                (long long)i, (long long)dim);
        abort();
    }
    int64_t ndim = a->ndim - 1;
    ZapArray *v = (ZapArray *)zap_obj(&zap_desc_array);
    int64_t *dims = (int64_t *)zap_buf((size_t)(ndim > 0 ? ndim : 1) * sizeof(int64_t));
    int64_t ri = 0;
    for (int64_t k = 0; k < a->ndim; k++) {
        if (k != axis) {
            dims[ri++] = a->dims[k];
        }
    }
    int64_t off = a->off + i * zap_array_stride_of(a, axis);
    /* A contiguous axis-0 index keeps a contiguous sub-block; otherwise the
     * surviving axes need explicit strides. */
    int64_t *strides = NULL;
    if (a->strides || axis != 0) {
        strides = (int64_t *)zap_buf((size_t)(ndim > 0 ? ndim : 1) * sizeof(int64_t));
        int64_t si = 0;
        for (int64_t k = 0; k < a->ndim; k++) {
            if (k != axis) {
                strides[si++] = zap_array_stride_of(a, k);
            }
        }
    }
    v->dtype = a->dtype;
    v->ndim = ndim;
    v->dims = dims;
    v->strides = strides;
    v->data = a->data;
    v->off = off;
    return v;
}

/* `m[i]` on a rank-n (n >= 2) array: the leading-axis integer index, kept as a
 * thin alias of `index_axis(a, 0, i)` for the codegen that still emits it. */
void *zap_array_row(void *ap, int64_t i) { return zap_array_index_axis(ap, 0, i); }

/* The raw element buffer at this array's first element; codegen's SIMD kernels
 * index this directly from 0. Valid only for a CONTIGUOUS array (strides NULL) —
 * the kernels pass every operand through `zap_array_contig` first (#502). */
void *zap_array_data(void *a) {
    ZapArray *arr = (ZapArray *)a;
    return (char *)arr->data + (size_t)arr->off * zap_array_elem_size(arr->dtype);
}

/* A contiguous 1-D slice `a[lo:hi]` (step 1) -> a NEW array header SHARING the
 * parent's buffer (D-0030): mutations through either alias. Python bounds
 * (negatives from the end, out-of-range clamps). Stepped / reversed / N-D slices
 * go through `zap_array_slice_axis` instead (#502). */
void *zap_array_slice(void *ap, int64_t lo, int64_t hi, int64_t has_lo, int64_t has_hi) {
    ZapArray *a = (ZapArray *)ap;
    int64_t n = a->dims[0];
    int64_t start = has_lo ? zap_slice_pos(lo, n) : 0;
    int64_t stop = has_hi ? zap_slice_pos(hi, n) : n;
    int64_t len = stop > start ? stop - start : 0;
    ZapArray *v = (ZapArray *)zap_obj(&zap_desc_array);
    int64_t *dims = (int64_t *)zap_buf(sizeof(int64_t));
    dims[0] = len;
    v->dtype = a->dtype;
    v->ndim = 1;
    v->dims = dims;
    /* A step-1 slice of a contiguous 1-D array stays contiguous; of a strided
     * view it inherits the parent's stride. */
    if (a->strides) {
        int64_t *st = (int64_t *)zap_buf(sizeof(int64_t));
        st[0] = a->strides[0];
        v->strides = st;
    } else {
        v->strides = NULL;
    }
    v->data = a->data;
    v->off = a->off + start * zap_array_stride_of(a, 0);
    return v;
}

/* Slice `axis` of `a`, KEEPING it (#502): the general basic-indexing primitive
 * for a stepped/reversed 1-D slice (`a[::2]`, `a[::-1]`), an N-D axis-0 window
 * (`m[lo:hi]`), or a sliced axis inside a multi-axis index (`m[:, j]` folds the
 * integer axes with `index_axis` and the sliced axes here). Python slice
 * semantics (negative bounds, sign-dependent defaults, clamping); a zero step is
 * a fatal error, matching list slicing. The surviving axis gets an explicit
 * stride (`parent_stride * step`), so the result is a strided view unless it is a
 * step-1 window of an already-contiguous axis-0. */
void *zap_array_slice_axis(void *ap, int64_t axis, int64_t lo, int64_t hi,
                           int64_t step, int64_t has_lo, int64_t has_hi,
                           int64_t has_step) {
    ZapArray *a = (ZapArray *)ap;
    int64_t n = a->dims[axis];
    int64_t st = has_step ? step : 1;
    if (st == 0) {
        zap_panic(zap_str_new("slice step cannot be zero", 25));
    }
    int64_t start, stop;
    if (st > 0) {
        start = has_lo ? zap_slice_pos(lo, n) : 0;
        stop = has_hi ? zap_slice_pos(hi, n) : n;
    } else {
        start = has_lo ? zap_slice_neg(lo, n) : n - 1;
        stop = has_hi ? zap_slice_neg(hi, n) : -1;
    }
    int64_t len = 0;
    if (st > 0) {
        len = stop > start ? (stop - start + st - 1) / st : 0;
    } else {
        len = start > stop ? (start - stop + (-st) - 1) / (-st) : 0;
    }
    int64_t pstride = zap_array_stride_of(a, axis);
    int64_t ndim = a->ndim;
    ZapArray *v = (ZapArray *)zap_obj(&zap_desc_array);
    int64_t *dims = (int64_t *)zap_buf((size_t)ndim * sizeof(int64_t));
    for (int64_t k = 0; k < ndim; k++) {
        dims[k] = a->dims[k];
    }
    dims[axis] = len;
    int64_t off = a->off + start * pstride;
    int64_t *strides = NULL;
    if (a->strides || st != 1 || axis != 0) {
        strides = (int64_t *)zap_buf((size_t)ndim * sizeof(int64_t));
        for (int64_t k = 0; k < ndim; k++) {
            strides[k] = zap_array_stride_of(a, k);
        }
        strides[axis] = pstride * st;
    }
    v->dtype = a->dtype;
    v->ndim = ndim;
    v->dims = dims;
    v->strides = strides;
    v->data = a->data;
    v->off = off;
    return v;
}

/* Out-of-range access aborts (shape-mismatch exceptions wait for a follow-up,
 * matching the list bounds behaviour above). */
/* The typed get/set bound is the TOTAL element count (product of all dims), not
 * `dims[0]`: these take a FLAT element index into the contiguous window. That is
 * how N-D construction fills the buffer (flat 0..count), and for a rank-1 array
 * or a rank-1 row view (the only shapes a full-index get/set ever lands on after
 * row folding) the count equals `dims[0]`, so 1-D behaviour is unchanged. A
 * partial index is bounds-checked per axis in `zap_array_row` instead. */
static void zap_array_bounds(ZapArray *a, int64_t i) {
    int64_t n = zap_array_count_of(a);
    if (i < 0 || i >= n) {
        fprintf(stderr, "zap: array index %lld out of range (len %lld)\n",
                (long long)i, (long long)n);
        abort();
    }
}

/* Validate both operands before a SIMD kernel reads either data buffer. The
 * element-wise kernels run flat over the full element count, so equal shapes
 * mean equal `ndim` and equal `dims` on every axis (#500; broadcasting across
 * unequal shapes is a follow-up, #501). A mismatch raises a catchable
 * `ValueError` (D-0007) before either buffer is read. */
void zap_array_check_shape(void *left, void *right) {
    ZapArray *a = (ZapArray *)left;
    ZapArray *b = (ZapArray *)right;
    int ok = a->ndim == b->ndim;
    for (int64_t k = 0; ok && k < a->ndim; k++) {
        ok = a->dims[k] == b->dims[k];
    }
    if (!ok) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("array shape mismatch", 20));
    }
}

/* ---------------------------------------------------------------------------
 * Broadcasting (#501, D-0165). NumPy semantics: shapes are right-aligned, and
 * each aligned axis must be equal, or one side 1 (stretched), or missing (the
 * lower-rank operand gets implicit leading 1s). The result rank is the larger
 * rank; each result dim is the larger of the two aligned dims.
 *
 * `a`'s length on result axis `k` (rank `r` = result rank): an axis past the
 * left edge of `a` is an implicit 1, as is a genuine size-1 (stretched) axis. */
static int64_t zap_array_bcast_dim(const ZapArray *a, int64_t r, int64_t k) {
    int64_t ax = k - (r - a->ndim);
    return ax < 0 ? 1 : a->dims[ax];
}

/* Window-relative contiguous row-major stride of `a`'s own axis `ax`
 * (product of the trailing dims); the element step when that axis increments. */
static int64_t zap_array_axis_stride(const ZapArray *a, int64_t ax) {
    int64_t s = 1;
    for (int64_t k = ax + 1; k < a->ndim; k++) {
        s *= a->dims[k];
    }
    return s;
}

int64_t zap_array_broadcastable(void *ap, void *bp) {
    ZapArray *a = (ZapArray *)ap;
    ZapArray *b = (ZapArray *)bp;
    int64_t r = a->ndim > b->ndim ? a->ndim : b->ndim;
    for (int64_t k = 0; k < r; k++) {
        int64_t da = zap_array_bcast_dim(a, r, k);
        int64_t db = zap_array_bcast_dim(b, r, k);
        if (da != db && da != 1 && db != 1) {
            return 0;
        }
    }
    return 1;
}

/* Raise a catchable ValueError when the operands cannot broadcast together
 * (#508 overlap). Emitted as a throwing call before the kernel, so the kernel
 * may assume compatibility. */
void zap_array_broadcast_check(void *ap, void *bp) {
    if (!zap_array_broadcastable(ap, bp)) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("array shape mismatch", 20));
    }
}

/* 1 iff the two arrays have identical ndim and dims (the fast, no-broadcast
 * path — the element buffers line up element-for-element). */
int64_t zap_array_same_shape(void *ap, void *bp) {
    ZapArray *a = (ZapArray *)ap;
    ZapArray *b = (ZapArray *)bp;
    if (a->ndim != b->ndim) {
        return 0;
    }
    for (int64_t k = 0; k < a->ndim; k++) {
        if (a->dims[k] != b->dims[k]) {
            return 0;
        }
    }
    return 1;
}

/* Allocate the element-wise result for a broadcast of `a` and `b` with dtype
 * `out_dtype`: rank = max rank, each dim = the larger aligned dim. Compatibility
 * must already have been checked (`zap_array_broadcast_check`). */
void *zap_array_broadcast_new(void *ap, void *bp, int64_t out_dtype) {
    ZapArray *a = (ZapArray *)ap;
    ZapArray *b = (ZapArray *)bp;
    int64_t r = a->ndim > b->ndim ? a->ndim : b->ndim;
    if (r < 1) {
        r = 1;
    }
    ZapArray *res = (ZapArray *)zap_obj(&zap_desc_array);
    int64_t *dims = (int64_t *)zap_buf((size_t)r * sizeof(int64_t));
    int64_t total = 1;
    for (int64_t k = 0; k < r; k++) {
        int64_t da = zap_array_bcast_dim(a, r, k);
        int64_t db = zap_array_bcast_dim(b, r, k);
        /* A size-1 axis stretches to the OTHER dim — which may be 0, so the
         * result is `da==1 ? db : da`, not `max(da, db)` (a `(0,)`/`(1,)` pair
         * broadcasts to an empty result, never a 1-element read of an empty
         * buffer). Compatibility (`da==db || da==1 || db==1`) is already checked. */
        dims[k] = (da == 1) ? db : da;
        total *= dims[k];
    }
    size_t esz = zap_array_elem_size(out_dtype);
    res->dtype = out_dtype;
    res->ndim = r;
    res->dims = dims;
    res->data = zap_buf((size_t)(total > 0 ? total : 0) * esz);
    res->off = 0;
    return res;
}

/* For a result flat index `f` into the broadcast result `res`, compute the
 * window-relative element offsets into `a` and `b` (stretched/missing axes
 * contribute 0, so they re-read the same element). The offsets are relative to
 * each operand's window base — the kernel's data pointers come from
 * `zap_array_data`, which already includes each array's `off`. */
void zap_array_bcast_offsets(void *ap, void *bp, void *resp, int64_t f,
                             int64_t *off_a, int64_t *off_b) {
    ZapArray *a = (ZapArray *)ap;
    ZapArray *b = (ZapArray *)bp;
    ZapArray *res = (ZapArray *)resp;
    int64_t r = res->ndim;
    int64_t oa = 0, ob = 0;
    int64_t rem = f;
    for (int64_t k = r - 1; k >= 0; k--) {
        int64_t idx = rem % res->dims[k];
        rem /= res->dims[k];
        int64_t aax = k - (r - a->ndim);
        if (aax >= 0 && a->dims[aax] != 1) {
            oa += idx * zap_array_axis_stride(a, aax);
        }
        int64_t bax = k - (r - b->ndim);
        if (bax >= 0 && b->dims[bax] != 1) {
            ob += idx * zap_array_axis_stride(b, bax);
        }
    }
    *off_a = oa;
    *off_b = ob;
}

/* The element step of operand `a` along the result's INNERMOST axis (#896): 0
 * when that axis is stretched (a size-1 or missing axis — the same value repeats
 * across the inner row, so the vectorized kernel splats it), else its contiguous
 * row-major stride, which for the last axis of a contiguous operand is 1 (the
 * kernel vector-loads). The broadcast kernel precomputes this once per operand,
 * then walks each inner row with `base + k * step` — no per-element offset call. */
int64_t zap_array_bcast_inner_stride(void *ap, void *resp) {
    ZapArray *a = (ZapArray *)ap;
    ZapArray *res = (ZapArray *)resp;
    int64_t r = res->ndim;
    int64_t aax = (r - 1) - (r - a->ndim); /* a's own last axis, == a->ndim - 1 */
    if (aax >= 0 && a->dims[aax] != 1)
        return zap_array_axis_stride(a, aax);
    return 0;
}

/* The typed get/set/sum for every numeric dtype (#49/#503/#890). Each integer
 * dtype crosses the ABI as an `int64_t` word: `get` sign/zero-extends the stored
 * element (the signedness of `ctype` picks which), `set` truncates (wraps mod
 * 2^width, D-0005), and `sum` accumulates in `int64_t` over the whole N-D
 * element window (#500). Each float dtype crosses as a `double`: `get` widens,
 * `set` narrows, `sum` accumulates in `double`. `bool` is special-cased below. */
/* Raised by `min`/`max` on an empty array (NumPy's "zero-size array to
 * reduction operation" — a catchable ValueError, #507). `sum`/`prod`/`mean`
 * have well-defined empty results (0 / 1 / nan) and never raise. */
static void zap_array_empty_reduce(void) {
    zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                    zap_str_new("reduction of an empty array", 27));
}

/* --- vectorized contiguous reducers (#911, D-0177) -----------------------
 * Each reducer runs an 8-way-unrolled body over the dense buffer: eight
 * independent accumulator lanes so the reduction is not a single latency-bound
 * dependency chain, which both hides FMA/add latency AND lets the compiler pack
 * the lanes into a SIMD vector at -O2. The lanes are horizontally combined,
 * then a scalar loop handles the final `n % 8` tail (so this also serves tiny
 * arrays, where `nb == 0` and only the tail runs).
 *
 * Numerics: integer add/mul is associative modulo 2^width, so integer sum/prod
 * are BIT-IDENTICAL to the old left-fold regardless of lane order. min/max are
 * order-independent for non-NaN data (float NaN handling stays unspecified, as
 * before). Floating-point sum/prod differ from the strict left-to-right fold
 * only by IEEE-754 reassociation — within tolerance, and the accepted cost of
 * vectorizing a bandwidth-bound reduction (#909). The accumulator TYPE is a
 * macro parameter because the public contracts differ: whole-array integer sum
 * widens to int64 and float reductions accumulate in double, while an axis=
 * reduction keeps the element width. Each caller instantiates and names the
 * helper it needs; min/max assume `n >= 1` (callers guard empty first). */
#define ZAP_SUMFN(name, ctype, accT)                                           \
    static accT name(const ctype *d, int64_t n) {                              \
        accT s0 = 0, s1 = 0, s2 = 0, s3 = 0, s4 = 0, s5 = 0, s6 = 0, s7 = 0;   \
        int64_t i = 0, nb = n & ~(int64_t)7;                                   \
        for (; i < nb; i += 8) {                                               \
            s0 += (accT)d[i];                                                  \
            s1 += (accT)d[i + 1];                                              \
            s2 += (accT)d[i + 2];                                              \
            s3 += (accT)d[i + 3];                                              \
            s4 += (accT)d[i + 4];                                              \
            s5 += (accT)d[i + 5];                                              \
            s6 += (accT)d[i + 6];                                              \
            s7 += (accT)d[i + 7];                                              \
        }                                                                      \
        accT s = ((s0 + s1) + (s2 + s3)) + ((s4 + s5) + (s6 + s7));            \
        for (; i < n; i++)                                                     \
            s += (accT)d[i];                                                   \
        return s;                                                              \
    }
#define ZAP_PRODFN(name, ctype, accT)                                          \
    static accT name(const ctype *d, int64_t n) {                              \
        accT p0 = 1, p1 = 1, p2 = 1, p3 = 1, p4 = 1, p5 = 1, p6 = 1, p7 = 1;   \
        int64_t i = 0, nb = n & ~(int64_t)7;                                   \
        for (; i < nb; i += 8) {                                               \
            p0 *= (accT)d[i];                                                  \
            p1 *= (accT)d[i + 1];                                              \
            p2 *= (accT)d[i + 2];                                              \
            p3 *= (accT)d[i + 3];                                              \
            p4 *= (accT)d[i + 4];                                              \
            p5 *= (accT)d[i + 5];                                              \
            p6 *= (accT)d[i + 6];                                              \
            p7 *= (accT)d[i + 7];                                              \
        }                                                                      \
        accT p = ((p0 * p1) * (p2 * p3)) * ((p4 * p5) * (p6 * p7));            \
        for (; i < n; i++)                                                     \
            p *= (accT)d[i];                                                   \
        return p;                                                              \
    }
#define ZAP_MINMAXFN(name, ctype, accT, CMP)                                   \
    static accT name(const ctype *d, int64_t n) {                              \
        accT m0 = (accT)d[0], m1 = m0, m2 = m0, m3 = m0, m4 = m0, m5 = m0,     \
             m6 = m0, m7 = m0;                                                 \
        int64_t i = 0, nb = n & ~(int64_t)7;                                   \
        for (; i < nb; i += 8) {                                               \
            accT v;                                                            \
            v = (accT)d[i];     if (v CMP m0) m0 = v;                          \
            v = (accT)d[i + 1]; if (v CMP m1) m1 = v;                          \
            v = (accT)d[i + 2]; if (v CMP m2) m2 = v;                          \
            v = (accT)d[i + 3]; if (v CMP m3) m3 = v;                          \
            v = (accT)d[i + 4]; if (v CMP m4) m4 = v;                          \
            v = (accT)d[i + 5]; if (v CMP m5) m5 = v;                          \
            v = (accT)d[i + 6]; if (v CMP m6) m6 = v;                          \
            v = (accT)d[i + 7]; if (v CMP m7) m7 = v;                          \
        }                                                                      \
        accT m = m0;                                                           \
        if (m1 CMP m) m = m1;                                                  \
        if (m2 CMP m) m = m2;                                                  \
        if (m3 CMP m) m = m3;                                                  \
        if (m4 CMP m) m = m4;                                                  \
        if (m5 CMP m) m = m5;                                                  \
        if (m6 CMP m) m = m6;                                                  \
        if (m7 CMP m) m = m7;                                                  \
        for (; i < n; i++) {                                                   \
            accT v = (accT)d[i];                                               \
            if (v CMP m) m = v;                                                \
        }                                                                      \
        return m;                                                              \
    }
/* The reducers an axis= path and both whole-array paths share: a width-keeping
 * sum (`csum`), a double sum for mean (`dsum`), and width-keeping prod/min/max.
 * Instantiated once per dtype for EVERY reducible suffix (incl. bool, whose
 * axis= path reduces its 0/1 bytes arithmetically in uint8, exactly as before).
 * The whole-array-only accumulators (int64 sum; double prod/min/max for floats)
 * are emitted inside the access macros so an unused one is never generated. */
#define ZAP_AXIS_REDUCE_HELPERS(suffix, ctype)                                 \
    ZAP_SUMFN(zap_csum_##suffix, ctype, ctype)                                 \
    ZAP_SUMFN(zap_dsum_##suffix, ctype, double)                                \
    ZAP_PRODFN(zap_prodc_##suffix, ctype, ctype)                               \
    ZAP_MINMAXFN(zap_minc_##suffix, ctype, ctype, <)                           \
    ZAP_MINMAXFN(zap_maxc_##suffix, ctype, ctype, >)

/* get/set are stride-aware (`zap_array_phys`) so a write through a strided view
 * aliases the parent; the reducers compact a strided view first
 * (`zap_array_contig`, a no-op when contiguous) so their inner loop stays a
 * dense `data + off` walk (#502). */
#define ZAP_ARRAY_INT_ACCESS(suffix, ctype)                                    \
    int64_t zap_array_get_##suffix(void *ap, int64_t i) {                      \
        ZapArray *a = (ZapArray *)ap;                                          \
        zap_array_bounds(a, i);                                                \
        return (int64_t)((const ctype *)a->data)[zap_array_phys(a, i)];        \
    }                                                                          \
    void zap_array_set_##suffix(void *ap, int64_t i, int64_t v) {             \
        ZapArray *a = (ZapArray *)ap;                                          \
        zap_array_bounds(a, i);                                               \
        ((ctype *)a->data)[zap_array_phys(a, i)] = (ctype)v;                  \
    }                                                                          \
    /* Whole-array integer sum widens to int64 (no per-width wrap), unlike the  \
     * axis= sum which keeps the element width. */                            \
    ZAP_SUMFN(zap_isum_##suffix, ctype, int64_t)                              \
    int64_t zap_array_sum_##suffix(void *ap) {                                \
        ZapArray *a = (ZapArray *)zap_array_contig(ap);                       \
        const ctype *d = (const ctype *)a->data + a->off;                     \
        return zap_isum_##suffix(d, zap_array_count_of(a));                   \
    }                                                                          \
    /* prod/min/max accumulate in `ctype` so each matches NumPy's per-dtype    \
     * result: an integer product wraps modulo 2^width, and min/max compare    \
     * with the operand's own signedness (widening an unsigned value to int64  \
     * would misorder values above INT64_MAX). The result crosses the ABI as   \
     * an int64 word, like get/sum. */                                         \
    int64_t zap_array_prod_##suffix(void *ap) {                               \
        ZapArray *a = (ZapArray *)zap_array_contig(ap);                       \
        const ctype *d = (const ctype *)a->data + a->off;                     \
        return (int64_t)zap_prodc_##suffix(d, zap_array_count_of(a));         \
    }                                                                          \
    int64_t zap_array_min_##suffix(void *ap) {                                \
        ZapArray *a = (ZapArray *)zap_array_contig(ap);                       \
        const ctype *d = (const ctype *)a->data + a->off;                     \
        int64_t n = zap_array_count_of(a);                                    \
        if (n == 0)                                                           \
            zap_array_empty_reduce();                                         \
        return (int64_t)zap_minc_##suffix(d, n);                              \
    }                                                                          \
    int64_t zap_array_max_##suffix(void *ap) {                                \
        ZapArray *a = (ZapArray *)zap_array_contig(ap);                       \
        const ctype *d = (const ctype *)a->data + a->off;                     \
        int64_t n = zap_array_count_of(a);                                    \
        if (n == 0)                                                           \
            zap_array_empty_reduce();                                         \
        return (int64_t)zap_maxc_##suffix(d, n);                              \
    }                                                                          \
    /* mean accumulates in double and returns f64 (empty -> nan, no raise);    \
     * the lowering narrows to f32 for an f32 source. */                       \
    double zap_array_mean_##suffix(void *ap) {                                \
        ZapArray *a = (ZapArray *)zap_array_contig(ap);                       \
        const ctype *d = (const ctype *)a->data + a->off;                     \
        int64_t n = zap_array_count_of(a);                                    \
        return zap_dsum_##suffix(d, n) / (double)n;                           \
    }

#define ZAP_ARRAY_FLOAT_ACCESS(suffix, ctype)                                  \
    double zap_array_get_##suffix(void *ap, int64_t i) {                       \
        ZapArray *a = (ZapArray *)ap;                                          \
        zap_array_bounds(a, i);                                                \
        return (double)((const ctype *)a->data)[zap_array_phys(a, i)];        \
    }                                                                          \
    void zap_array_set_##suffix(void *ap, int64_t i, double v) {              \
        ZapArray *a = (ZapArray *)ap;                                         \
        zap_array_bounds(a, i);                                               \
        ((ctype *)a->data)[zap_array_phys(a, i)] = (ctype)v;                  \
    }                                                                          \
    /* Float reductions accumulate in double (so an f32 sum does not lose      \
     * precision), returning f64; the lowering narrows to f32 for an f32       \
     * source. The axis= sum keeps the element width (see the axis helpers). */ \
    ZAP_PRODFN(zap_dprod_##suffix, ctype, double)                             \
    ZAP_MINMAXFN(zap_dmin_##suffix, ctype, double, <)                         \
    ZAP_MINMAXFN(zap_dmax_##suffix, ctype, double, >)                         \
    double zap_array_sum_##suffix(void *ap) {                                 \
        ZapArray *a = (ZapArray *)zap_array_contig(ap);                       \
        const ctype *d = (const ctype *)a->data + a->off;                     \
        return zap_dsum_##suffix(d, zap_array_count_of(a));                   \
    }                                                                          \
    double zap_array_prod_##suffix(void *ap) {                                \
        ZapArray *a = (ZapArray *)zap_array_contig(ap);                       \
        const ctype *d = (const ctype *)a->data + a->off;                     \
        return zap_dprod_##suffix(d, zap_array_count_of(a));                  \
    }                                                                          \
    double zap_array_min_##suffix(void *ap) {                                 \
        ZapArray *a = (ZapArray *)zap_array_contig(ap);                       \
        const ctype *d = (const ctype *)a->data + a->off;                     \
        int64_t n = zap_array_count_of(a);                                    \
        if (n == 0)                                                           \
            zap_array_empty_reduce();                                         \
        return zap_dmin_##suffix(d, n);                                       \
    }                                                                          \
    double zap_array_max_##suffix(void *ap) {                                 \
        ZapArray *a = (ZapArray *)zap_array_contig(ap);                       \
        const ctype *d = (const ctype *)a->data + a->off;                     \
        int64_t n = zap_array_count_of(a);                                    \
        if (n == 0)                                                           \
            zap_array_empty_reduce();                                         \
        return zap_dmax_##suffix(d, n);                                       \
    }                                                                          \
    double zap_array_mean_##suffix(void *ap) {                                \
        ZapArray *a = (ZapArray *)zap_array_contig(ap);                       \
        const ctype *d = (const ctype *)a->data + a->off;                     \
        int64_t n = zap_array_count_of(a);                                    \
        return zap_dsum_##suffix(d, n) / (double)n;                           \
    }

/* The shared width-keeping reducers, one set per dtype (bool's set drives its
 * axis= path, which reduces 0/1 bytes arithmetically in uint8 — unchanged). */
ZAP_AXIS_REDUCE_HELPERS(f64, double)
ZAP_AXIS_REDUCE_HELPERS(f32, float)
ZAP_AXIS_REDUCE_HELPERS(i64, int64_t)
ZAP_AXIS_REDUCE_HELPERS(i32, int32_t)
ZAP_AXIS_REDUCE_HELPERS(i16, int16_t)
ZAP_AXIS_REDUCE_HELPERS(i8, int8_t)
ZAP_AXIS_REDUCE_HELPERS(u8, uint8_t)
ZAP_AXIS_REDUCE_HELPERS(u16, uint16_t)
ZAP_AXIS_REDUCE_HELPERS(u32, uint32_t)
ZAP_AXIS_REDUCE_HELPERS(u64, uint64_t)
ZAP_AXIS_REDUCE_HELPERS(bool, uint8_t)

ZAP_ARRAY_FLOAT_ACCESS(f64, double)
ZAP_ARRAY_FLOAT_ACCESS(f32, float)
ZAP_ARRAY_INT_ACCESS(i64, int64_t)
ZAP_ARRAY_INT_ACCESS(i32, int32_t)
ZAP_ARRAY_INT_ACCESS(i16, int16_t)
ZAP_ARRAY_INT_ACCESS(i8, int8_t)
ZAP_ARRAY_INT_ACCESS(u8, uint8_t)
ZAP_ARRAY_INT_ACCESS(u16, uint16_t)
ZAP_ARRAY_INT_ACCESS(u32, uint32_t)
ZAP_ARRAY_INT_ACCESS(u64, uint64_t)

/* `bool` is a 0/1 byte: `set` normalizes any nonzero to 1, `get`/`sum` read it
 * back as 0/1 (distinct from `uint8`, which stores the raw byte). */
int64_t zap_array_get_bool(void *ap, int64_t i) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    return ((const uint8_t *)a->data)[zap_array_phys(a, i)] ? 1 : 0;
}
void zap_array_set_bool(void *ap, int64_t i, int64_t v) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    ((uint8_t *)a->data)[zap_array_phys(a, i)] = v ? 1 : 0;
}
int64_t zap_array_sum_bool(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    const uint8_t *d = (const uint8_t *)a->data + a->off;
    int64_t n = zap_array_count_of(a);
    int64_t s = 0;
    for (int64_t i = 0; i < n; i++) {
        s += d[i] ? 1 : 0;
    }
    return s;
}

/* Forward declarations: the generic element load/store are defined further down
 * (with the matmul kernels), but the complex-array helpers below read a real
 * operand through `mm_load_f` and store an f64 magnitude through `arr_store_f`. */
static double mm_load_f(const ZapArray *a, int64_t i);
static void arr_store_f(ZapArray *a, int64_t i, double v);

/* --- complex128 arrays (#938) --------------------------------------------
 * A complex element is 16 bytes: two contiguous f64 lanes (re, im). Unlike the
 * scalar dtypes, a complex element is not a single SIMD lane, so the lowering
 * routes EVERY complex-array operation here rather than through the inline
 * element-wise kernel. get/set are stride-aware (the physical element index from
 * `zap_array_phys` selects a 2-double slot); `a[i]` materialises a heap
 * `ZapComplex`. */
void *zap_array_get_complex(void *ap, int64_t i) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    int64_t p = zap_array_phys(a, i);
    const double *d = (const double *)a->data;
    return zap_complex_new(d[2 * p], d[2 * p + 1]);
}
void zap_array_set_complex(void *ap, int64_t i, void *zp) {
    ZapArray *a = (ZapArray *)ap;
    zap_array_bounds(a, i);
    int64_t p = zap_array_phys(a, i);
    const ZapComplex *z = (const ZapComplex *)zp;
    double *d = (double *)a->data;
    d[2 * p] = z->re;
    d[2 * p + 1] = z->im;
}
/* Read element `i` (flat; caller has contiguated) of ANY numeric array as a
 * complex pair: a real dtype yields (value, 0); a complex dtype reads both
 * lanes. Lets element-wise ops mix a real and a complex operand. */
static void zap_cx_load(const ZapArray *a, int64_t i, double *re, double *im) {
    if (a->dtype == DTYPE_COMPLEX) {
        const double *d = (const double *)a->data + 2 * a->off;
        *re = d[2 * i];
        *im = d[2 * i + 1];
    } else {
        *re = mm_load_f(a, i);
        *im = 0.0;
    }
}
/* Store into a freshly allocated (contiguous, off == 0) complex result. */
static void zap_cx_store(ZapArray *res, int64_t i, double re, double im) {
    double *d = (double *)res->data;
    d[2 * i] = re;
    d[2 * i + 1] = im;
}
static void zap_cx_op(int64_t op, double ar, double ai, double br, double bi,
                      double *rr, double *ri) {
    switch (op) {
    case 0: /* add */
        *rr = ar + br;
        *ri = ai + bi;
        break;
    case 1: /* sub */
        *rr = ar - br;
        *ri = ai - bi;
        break;
    case 2: /* mul */
        *rr = ar * br - ai * bi;
        *ri = ar * bi + ai * br;
        break;
    default: { /* div */
        double den = br * br + bi * bi;
        *rr = (ar * br + ai * bi) / den;
        *ri = (ai * br - ar * bi) / den;
    }
    }
}
static ZapArray *zap_cx_alloc_like(const ZapArray *a, int64_t dtype) {
    int64_t n = zap_array_count_of(a);
    ZapArray *res = (ZapArray *)zap_array_new_nd(dtype, a->ndim, n);
    for (int64_t k = 0; k < a->ndim; k++)
        zap_array_set_dim(res, k, a->dims[k]);
    return res;
}
static int zap_cx_same_shape(const ZapArray *a, const ZapArray *b) {
    if (a->ndim != b->ndim)
        return 0;
    for (int64_t k = 0; k < a->ndim; k++)
        if (a->dims[k] != b->dims[k])
            return 0;
    return 1;
}
/* array ⊗ array -> complex array; either operand may be a real dtype (promotes
 * to complex) or complex. First cut: equal shapes only — the lowering's
 * `zap_array_broadcast_check` has rejected rank/axis incompatibility, and this
 * refuses an unequal (dim-stretch) shape rather than miscompute (complex
 * broadcasting is a follow-up). op: 0 add, 1 sub, 2 mul, 3 div. */
void *zap_array_cbinop(void *ap, void *bp, int64_t op) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    ZapArray *b = (ZapArray *)zap_array_contig(bp);
    if (!zap_cx_same_shape(a, b))
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("complex array element-wise op requires equal "
                                    "shapes (broadcasting is not supported yet)",
                                    75));
    int64_t n = zap_array_count_of(a);
    ZapArray *res = zap_cx_alloc_like(a, DTYPE_COMPLEX);
    for (int64_t i = 0; i < n; i++) {
        double ar, ai, br, bi, rr, ri;
        zap_cx_load(a, i, &ar, &ai);
        zap_cx_load(b, i, &br, &bi);
        zap_cx_op(op, ar, ai, br, bi, &rr, &ri);
        zap_cx_store(res, i, rr, ri);
    }
    return res;
}
/* array ⊗ complex-scalar -> complex array. `scalar_lhs` computes `scalar op
 * a[i]` (the non-commutative - and /); else `a[i] op scalar`. A real scalar is
 * passed as a `ZapComplex` with a zero imaginary part by the lowering. */
void *zap_array_cbinop_cs(void *ap, void *zp, int64_t op, int64_t scalar_lhs) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    const ZapComplex *z = (const ZapComplex *)zp;
    int64_t n = zap_array_count_of(a);
    ZapArray *res = zap_cx_alloc_like(a, DTYPE_COMPLEX);
    for (int64_t i = 0; i < n; i++) {
        double ar, ai, rr, ri;
        zap_cx_load(a, i, &ar, &ai);
        if (scalar_lhs)
            zap_cx_op(op, z->re, z->im, ar, ai, &rr, &ri);
        else
            zap_cx_op(op, ar, ai, z->re, z->im, &rr, &ri);
        zap_cx_store(res, i, rr, ri);
    }
    return res;
}
/* conj(a) -> complex array (negates the imaginary lane; a real input lifts to a
 * complex array with a zero imaginary part). */
void *zap_array_conj(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_array_count_of(a);
    ZapArray *res = zap_cx_alloc_like(a, DTYPE_COMPLEX);
    for (int64_t i = 0; i < n; i++) {
        double re, im;
        zap_cx_load(a, i, &re, &im);
        zap_cx_store(res, i, re, -im);
    }
    return res;
}
/* abs(a) -> f64 array of magnitudes sqrt(re^2 + im^2). */
void *zap_array_cabs(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_array_count_of(a);
    ZapArray *res = zap_cx_alloc_like(a, DTYPE_F64);
    for (int64_t i = 0; i < n; i++) {
        double re, im;
        zap_cx_load(a, i, &re, &im);
        arr_store_f(res, i, sqrt(re * re + im * im));
    }
    return res;
}
/* -a (unary negation) -> complex array (negate both lanes; a real input lifts to
 * complex with a zero imaginary part). */
void *zap_array_cneg(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_array_count_of(a);
    ZapArray *res = zap_cx_alloc_like(a, DTYPE_COMPLEX);
    for (int64_t i = 0; i < n; i++) {
        double re, im;
        zap_cx_load(a, i, &re, &im);
        zap_cx_store(res, i, -re, -im);
    }
    return res;
}
/* Whole-array complex sum -> a complex scalar. */
void *zap_array_sum_complex(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t n = zap_array_count_of(a);
    double sr = 0.0, si = 0.0;
    for (int64_t i = 0; i < n; i++) {
        double re, im;
        zap_cx_load(a, i, &re, &im);
        sr += re;
        si += im;
    }
    return zap_complex_new(sr, si);
}
/* --- FFT (#929), vendored PocketFFT backend (crates/codegen/src/zapfft.cpp) --
 * The public `zap_fft_*` entry points are ALWAYS compiled (so a program that
 * imports `fft` links), but the PocketFFT-backed transforms need the C++ backend
 * object, linked only for such a program (`BackendUse::fft` ->
 * `-DZAP_HAVE_FFT` + the C++ `zapfft.o` + the C++ stdlib). A build without the
 * gate raises a catchable ValueError (never reached in practice — reaching `fft`
 * sets the gate). `norm` is 0 = backward (default), 1 = ortho, 2 = forward. All
 * transforms output `complex` arrays (rfft's half spectrum, irfft a real array);
 * `fftfreq`/`rfftfreq` are pure arithmetic and need no backend. */
static void zap_fft_err(const char *m) {
    zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
}
#if defined(ZAP_HAVE_FFT)
extern int zapfft_c2c(const double *in, double *out, const size_t *shape,
                      size_t ndim, const size_t *axes, size_t naxes, int forward,
                      double fct);
extern int zapfft_r2c(const double *in, double *out, size_t n, int forward,
                      double fct);
extern int zapfft_c2r(const double *in, double *out, size_t n, int forward,
                      double fct);
/* The normalization scale (NumPy `norm`): backward scales the inverse by 1/N,
 * forward scales the forward by 1/N, ortho scales both by 1/sqrt(N). */
static double zap_fft_norm_fct(double nn, int inv, int64_t norm) {
    if (norm == 1)
        return 1.0 / sqrt(nn);
    if (norm == 2)
        return inv ? 1.0 : 1.0 / nn;
    return inv ? 1.0 / nn : 1.0;
}
/* Materialize `a` (real or complex) into a fresh interleaved (re, im) buffer. */
static double *zap_fft_to_cbuf(ZapArray *a, int64_t n) {
    double *buf = (double *)zap_buf((size_t)(n > 0 ? n : 1) * 2 * sizeof(double));
    for (int64_t i = 0; i < n; i++) {
        double re, im;
        zap_cx_load(a, i, &re, &im);
        buf[2 * i] = re;
        buf[2 * i + 1] = im;
    }
    return buf;
}
/* c2c over `axes` (the shared core of fft/ifft/fft2/ifft2/fftn/ifftn). The input
 * promotes to complex; the result is a complex array of the same shape. */
static void *zap_fft_c2c_core(void *ap, const int64_t *axes, int64_t naxes,
                              int inv, int64_t norm) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    int64_t nd = a->ndim;
    int64_t cnt = zap_array_count_of(a);
    double *inbuf = zap_fft_to_cbuf(a, cnt);
    ZapArray *res = (ZapArray *)zap_array_new_nd(DTYPE_COMPLEX, nd, cnt);
    for (int64_t k = 0; k < nd; k++)
        zap_array_set_dim(res, k, a->dims[k]);
    size_t *shp = (size_t *)zap_buf((size_t)(nd > 0 ? nd : 1) * sizeof(size_t));
    for (int64_t k = 0; k < nd; k++)
        shp[k] = (size_t)a->dims[k];
    size_t *ax = (size_t *)zap_buf((size_t)(naxes > 0 ? naxes : 1) * sizeof(size_t));
    double nn = 1.0;
    for (int64_t k = 0; k < naxes; k++) {
        ax[k] = (size_t)axes[k];
        nn *= (double)a->dims[axes[k]];
    }
    double fct = zap_fft_norm_fct(nn, inv, norm);
    int e = zapfft_c2c(inbuf, (double *)res->data, shp, (size_t)nd, ax,
                       (size_t)naxes, inv ? 0 : 1, fct);
    if (e)
        zap_fft_err("fft: transform failed (invalid shape)");
    return res;
}
static void *zap_fft_rfft_impl(ZapArray *a, int64_t norm) {
    int64_t n = a->dims[0];
    int64_t m = n / 2 + 1;
    double *inbuf = (double *)zap_buf((size_t)(n > 0 ? n : 1) * sizeof(double));
    for (int64_t i = 0; i < n; i++) {
        double re, im;
        zap_cx_load(a, i, &re, &im);
        inbuf[i] = re;
    }
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_COMPLEX, m);
    double fct = zap_fft_norm_fct((double)n, 0, norm);
    int e = zapfft_r2c(inbuf, (double *)res->data, (size_t)n, 1, fct);
    if (e)
        zap_fft_err("rfft: transform failed");
    return res;
}
static void *zap_fft_irfft_impl(ZapArray *a, int64_t n, int64_t norm) {
    int64_t m = a->dims[0]; /* complex input length = n/2 + 1 */
    if (n <= 0)
        n = 2 * (m - 1); /* NumPy default output length */
    double *inbuf = zap_fft_to_cbuf(a, m);
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_F64, n);
    double fct = zap_fft_norm_fct((double)n, 1, norm);
    int e = zapfft_c2r(inbuf, (double *)res->data, (size_t)n, 0, fct);
    if (e)
        zap_fft_err("irfft: transform failed");
    return res;
}
#endif /* ZAP_HAVE_FFT */

#if defined(ZAP_HAVE_FFT)
#define ZAP_FFT_NEED_BACKEND 0
#else
#define ZAP_FFT_NEED_BACKEND 1
static void zap_fft_no_backend(void) {
    zap_fft_err("fft requires the FFT backend (this build has none)");
}
#endif

/* c2c public entry points: fft/ifft transform the LAST axis, fft2/ifft2 the last
 * two, fftn/ifftn all axes (matching numpy.fft's default axis selection). */
void *zap_fft_fft(void *ap, int64_t norm) {
    ZapArray *a = (ZapArray *)ap;
    if (a->ndim < 1)
        zap_fft_err("fft: expected an array");
#if ZAP_FFT_NEED_BACKEND
    (void)norm;
    zap_fft_no_backend();
    return NULL;
#else
    int64_t ax = a->ndim - 1;
    return zap_fft_c2c_core(ap, &ax, 1, 0, norm);
#endif
}
void *zap_fft_ifft(void *ap, int64_t norm) {
    ZapArray *a = (ZapArray *)ap;
    if (a->ndim < 1)
        zap_fft_err("ifft: expected an array");
#if ZAP_FFT_NEED_BACKEND
    (void)norm;
    zap_fft_no_backend();
    return NULL;
#else
    int64_t ax = a->ndim - 1;
    return zap_fft_c2c_core(ap, &ax, 1, 1, norm);
#endif
}
void *zap_fft_fft2(void *ap, int64_t norm) {
    ZapArray *a = (ZapArray *)ap;
    if (a->ndim < 2)
        zap_fft_err("fft2: expected a >= 2-D array");
#if ZAP_FFT_NEED_BACKEND
    (void)norm;
    zap_fft_no_backend();
    return NULL;
#else
    int64_t ax[2] = {a->ndim - 2, a->ndim - 1};
    return zap_fft_c2c_core(ap, ax, 2, 0, norm);
#endif
}
void *zap_fft_ifft2(void *ap, int64_t norm) {
    ZapArray *a = (ZapArray *)ap;
    if (a->ndim < 2)
        zap_fft_err("ifft2: expected a >= 2-D array");
#if ZAP_FFT_NEED_BACKEND
    (void)norm;
    zap_fft_no_backend();
    return NULL;
#else
    int64_t ax[2] = {a->ndim - 2, a->ndim - 1};
    return zap_fft_c2c_core(ap, ax, 2, 1, norm);
#endif
}
void *zap_fft_fftn(void *ap, int64_t norm) {
    ZapArray *a = (ZapArray *)ap;
    if (a->ndim < 1)
        zap_fft_err("fftn: expected an array");
#if ZAP_FFT_NEED_BACKEND
    (void)norm;
    zap_fft_no_backend();
    return NULL;
#else
    int64_t nd = a->ndim;
    int64_t *ax = (int64_t *)zap_buf((size_t)(nd > 0 ? nd : 1) * sizeof(int64_t));
    for (int64_t k = 0; k < nd; k++)
        ax[k] = k;
    return zap_fft_c2c_core(ap, ax, nd, 0, norm);
#endif
}
void *zap_fft_ifftn(void *ap, int64_t norm) {
    ZapArray *a = (ZapArray *)ap;
    if (a->ndim < 1)
        zap_fft_err("ifftn: expected an array");
#if ZAP_FFT_NEED_BACKEND
    (void)norm;
    zap_fft_no_backend();
    return NULL;
#else
    int64_t nd = a->ndim;
    int64_t *ax = (int64_t *)zap_buf((size_t)(nd > 0 ? nd : 1) * sizeof(int64_t));
    for (int64_t k = 0; k < nd; k++)
        ax[k] = k;
    return zap_fft_c2c_core(ap, ax, nd, 1, norm);
#endif
}
void *zap_fft_rfft(void *ap, int64_t norm) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    if (a->ndim != 1)
        zap_fft_err("rfft: expected a 1-D array");
#if ZAP_FFT_NEED_BACKEND
    (void)norm;
    zap_fft_no_backend();
    return NULL;
#else
    return zap_fft_rfft_impl(a, norm);
#endif
}
void *zap_fft_irfft(void *ap, int64_t n, int64_t norm) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    if (a->ndim != 1)
        zap_fft_err("irfft: expected a 1-D array");
#if ZAP_FFT_NEED_BACKEND
    (void)n;
    (void)norm;
    zap_fft_no_backend();
    return NULL;
#else
    return zap_fft_irfft_impl(a, n, norm);
#endif
}
/* Sample frequencies — pure arithmetic, no backend. fftfreq: n bins, the
 * non-negative then negative half; rfftfreq: n/2+1 non-negative bins. */
void *zap_fft_fftfreq(int64_t n, double d) {
    if (n <= 0)
        zap_fft_err("fftfreq: n must be positive");
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_F64, n);
    double val = 1.0 / ((double)n * d);
    for (int64_t i = 0; i < n; i++) {
        int64_t f = (i < (n + 1) / 2) ? i : i - n;
        arr_store_f(res, i, (double)f * val);
    }
    return res;
}
void *zap_fft_rfftfreq(int64_t n, double d) {
    if (n <= 0)
        zap_fft_err("rfftfreq: n must be positive");
    int64_t m = n / 2 + 1;
    ZapArray *res = (ZapArray *)zap_array_new(DTYPE_F64, m);
    double val = 1.0 / ((double)n * d);
    for (int64_t i = 0; i < m; i++)
        arr_store_f(res, i, (double)i * val);
    return res;
}

/* bool reductions (#507): min/max/prod stay 0/1 (prod = logical "all"); mean is
 * the fraction of True, a float. Elements are normalized 0/1 bytes. */
int64_t zap_array_prod_bool(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    const uint8_t *d = (const uint8_t *)a->data + a->off;
    int64_t n = zap_array_count_of(a);
    for (int64_t i = 0; i < n; i++)
        if (!d[i])
            return 0;
    return 1;
}
int64_t zap_array_min_bool(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    const uint8_t *d = (const uint8_t *)a->data + a->off;
    int64_t n = zap_array_count_of(a);
    if (n == 0)
        zap_array_empty_reduce();
    for (int64_t i = 0; i < n; i++)
        if (!d[i])
            return 0;
    return 1;
}
int64_t zap_array_max_bool(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    const uint8_t *d = (const uint8_t *)a->data + a->off;
    int64_t n = zap_array_count_of(a);
    if (n == 0)
        zap_array_empty_reduce();
    for (int64_t i = 0; i < n; i++)
        if (d[i])
            return 1;
    return 0;
}
double zap_array_mean_bool(void *ap) {
    ZapArray *a = (ZapArray *)zap_array_contig(ap);
    const uint8_t *d = (const uint8_t *)a->data + a->off;
    int64_t n = zap_array_count_of(a);
    int64_t s = 0;
    for (int64_t i = 0; i < n; i++)
        s += d[i] ? 1 : 0;
    return (double)s / (double)n;
}

/* --- axis reductions (#507, D-0170) --------------------------------------
 * `m.sum(axis=k)` and friends reduce one axis of an N-D array, producing a
 * rank-(ndim-1) result (the lowering handles a rank-1 source as a full, scalar
 * reduction, so these run only for ndim >= 2). The contiguous row-major buffer
 * factors around the reduced axis into `outer * axislen * inner`: element
 * (o, k, j) lives at flat index `(o*axislen + k)*inner + j`, and the result
 * element (o, j) at `o*inner + j`. `op` is 0=sum, 1=min, 2=max, 3=prod; mean
 * has its own entry point because its result is always a float. sum/min/max/
 * prod keep the input dtype and accumulate in its own `ctype` (so an integer
 * product wraps per width and unsigned min/max order correctly).
 *
 * Vectorization (#911, D-0177) picks the kernel from the layout, and either way
 * every output cell keeps the SAME per-element accumulation order as the scalar
 * reference, so results are BIT-IDENTICAL for every dtype:
 *  - inner == 1 (reducing the LAST axis): each output cell reduces a contiguous
 *    run, so the unrolled contiguous reducers apply directly.
 *  - inner  > 1: interchange the k (reduced) and j (contiguous) loops, so the
 *    innermost work is a contiguous element-wise accumulate over `inner` lanes
 *    into the output row — which the compiler vectorizes. `in`/`out` are
 *    distinct allocations (a compacted source vs a fresh result), so they are
 *    marked `restrict` to let the compiler assume no aliasing. */
#define ZAP_REDUCE_AXIS(suffix, ctype)                                         \
    static void zap_reduce_axis_##suffix(const ctype *restrict in,             \
                                         ctype *restrict out, int64_t outer,    \
                                         int64_t axislen, int64_t inner,        \
                                         int64_t op) {                          \
        if (inner == 1) {                                                      \
            for (int64_t o = 0; o < outer; o++) {                              \
                const ctype *col = in + o * axislen;                           \
                if (op == 3)                                                   \
                    out[o] = zap_prodc_##suffix(col, axislen);                 \
                else if (op == 0)                                              \
                    out[o] = zap_csum_##suffix(col, axislen);                  \
                else if (op == 1)                                             \
                    out[o] = zap_minc_##suffix(col, axislen);                  \
                else                                                           \
                    out[o] = zap_maxc_##suffix(col, axislen);                  \
            }                                                                  \
            return;                                                            \
        }                                                                      \
        for (int64_t o = 0; o < outer; o++) {                                  \
            ctype *orow = out + o * inner;                                     \
            const ctype *base = in + (o * axislen) * inner;                    \
            if (op == 0) { /* sum */                                           \
                for (int64_t j = 0; j < inner; j++)                            \
                    orow[j] = (ctype)0;                                        \
                for (int64_t k = 0; k < axislen; k++) {                        \
                    const ctype *kr = base + k * inner;                        \
                    for (int64_t j = 0; j < inner; j++)                        \
                        orow[j] += kr[j];                                      \
                }                                                              \
            } else if (op == 3) { /* prod */                                   \
                for (int64_t j = 0; j < inner; j++)                            \
                    orow[j] = (ctype)1;                                        \
                for (int64_t k = 0; k < axislen; k++) {                        \
                    const ctype *kr = base + k * inner;                        \
                    for (int64_t j = 0; j < inner; j++)                        \
                        orow[j] *= kr[j];                                      \
                }                                                              \
            } else if (op == 1) { /* min; axislen >= 1 (guarded) */            \
                for (int64_t j = 0; j < inner; j++)                            \
                    orow[j] = base[j];                                         \
                for (int64_t k = 1; k < axislen; k++) {                        \
                    const ctype *kr = base + k * inner;                        \
                    for (int64_t j = 0; j < inner; j++)                        \
                        if (kr[j] < orow[j])                                   \
                            orow[j] = kr[j];                                   \
                }                                                              \
            } else { /* max; axislen >= 1 (guarded) */                         \
                for (int64_t j = 0; j < inner; j++)                            \
                    orow[j] = base[j];                                         \
                for (int64_t k = 1; k < axislen; k++) {                        \
                    const ctype *kr = base + k * inner;                        \
                    for (int64_t j = 0; j < inner; j++)                        \
                        if (kr[j] > orow[j])                                   \
                            orow[j] = kr[j];                                   \
                }                                                              \
            }                                                                  \
        }                                                                      \
    }                                                                          \
    static void zap_mean_axis_##suffix(const ctype *restrict in,               \
                                       double *restrict out, int64_t outer,     \
                                       int64_t axislen, int64_t inner) {        \
        if (inner == 1) {                                                      \
            for (int64_t o = 0; o < outer; o++)                                \
                out[o] =                                                        \
                    zap_dsum_##suffix(in + o * axislen, axislen) /             \
                    (double)axislen;                                           \
            return;                                                            \
        }                                                                      \
        for (int64_t o = 0; o < outer; o++) {                                  \
            double *orow = out + o * inner;                                    \
            const ctype *base = in + (o * axislen) * inner;                    \
            for (int64_t j = 0; j < inner; j++)                                \
                orow[j] = 0.0;                                                 \
            for (int64_t k = 0; k < axislen; k++) {                            \
                const ctype *kr = base + k * inner;                            \
                for (int64_t j = 0; j < inner; j++)                            \
                    orow[j] += (double)kr[j];                                  \
            }                                                                  \
            for (int64_t j = 0; j < inner; j++)                                \
                orow[j] /= (double)axislen;                                    \
        }                                                                      \
    }

ZAP_REDUCE_AXIS(f64, double)
ZAP_REDUCE_AXIS(f32, float)
ZAP_REDUCE_AXIS(i64, int64_t)
ZAP_REDUCE_AXIS(i32, int32_t)
ZAP_REDUCE_AXIS(i16, int16_t)
ZAP_REDUCE_AXIS(i8, int8_t)
ZAP_REDUCE_AXIS(u8, uint8_t)
ZAP_REDUCE_AXIS(u16, uint16_t)
ZAP_REDUCE_AXIS(u32, uint32_t)
ZAP_REDUCE_AXIS(u64, uint64_t)
ZAP_REDUCE_AXIS(bool, uint8_t)

/* Normalize a (possibly negative) axis and raise a catchable ValueError if it
 * is out of range for `ndim` (#507). Returns the non-negative axis. */
static int64_t zap_array_axis_norm(int64_t axis, int64_t ndim) {
    if (axis < 0)
        axis += ndim;
    if (axis < 0 || axis >= ndim) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("array axis out of range", 24));
    }
    return axis;
}

/* Allocate the rank-(ndim-1) result of reducing `a` along `axis`, with dims
 * copied from `a` minus the reduced axis. `*outer`/`*axislen`/`*inner` receive
 * the factorization. The result dtype is `out_dtype`. */
static ZapArray *zap_array_reduce_alloc(ZapArray *a, int64_t axis,
                                        int64_t out_dtype, int64_t *outer,
                                        int64_t *axislen, int64_t *inner) {
    int64_t nd = a->ndim;
    int64_t ox = 1, ax = a->dims[axis], in = 1;
    for (int64_t k = 0; k < axis; k++)
        ox *= a->dims[k];
    for (int64_t k = axis + 1; k < nd; k++)
        in *= a->dims[k];
    *outer = ox;
    *axislen = ax;
    *inner = in;
    ZapArray *res = (ZapArray *)zap_array_new_nd(out_dtype, nd - 1, ox * in);
    int64_t ri = 0;
    for (int64_t k = 0; k < nd; k++)
        if (k != axis)
            zap_array_set_dim(res, ri++, a->dims[k]);
    return res;
}

/* Validate a (possibly negative) `axis` against an array's rank, raising a
 * catchable ValueError if out of range (#507). Used by the rank-1 full-reduction
 * path, where the reduction result is a scalar but an out-of-range axis must
 * still fault like the N-D `axis=` forms. */
void zap_array_axis_check(void *ap, int64_t axis) {
    (void)zap_array_axis_norm(axis, ((ZapArray *)ap)->ndim);
}

#include "runtime/array.inc.c"
#include "runtime/str.inc.c"
/* ===========================================================================
 * Coroutines + channels on an M:N work-stealing scheduler
 * (ticket #47, D-0008/D-0013/D-0031/D-0033/D-0034, design/0005-concurrency.md).
 *
 * This replaces the 1:1 pthread-backed coroutine runtime shipped in #25/#46
 * with a user-space scheduler. Language-visible semantics are UNCHANGED; this
 * is purely a runtime swap, and every `zap_*` entry point below keeps the exact
 * signature #25 compiled against, so lowering/codegen are untouched (the one
 * additive codegen change is the spawn trampoline's landing pad, which only
 * affects the uncaught-in-coroutine path).
 *
 * The Go M:P:G structure:
 *   - G  a coroutine: a fixed-size green-thread stack + a saved register
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
 * A *running* G executes on its green stack on some M. Boehm scans a stopped
 * thread as `[sp, stack_end)`; to keep that from being a bogus cross-stack range
 * while a G runs, the runtime installs a `GC_set_sp_corrector` hook (see
 * `zap_sp_corrector`, I#885) and never moves `stack_end` off the M's native stack
 * top — the running G's stack is scanned as heap through its `G` instead.
 *
 * Context switch (design/0005 "Interaction with exception unwinding"):
 * ucontext `swapcontext` saves/restores callee-saved regs + SP + IP. ARCH
 * NOTE: ucontext is available and correct on glibc Linux for both x86_64 and
 * aarch64 (both exercised in CI, ticket #204: the `full` and `arm64` jobs). The spawn trampoline is each G's base frame; an
 * uncaught exception is caught there by a landing pad (codegen), so unwinding
 * never crosses into the makecontext bootstrap. A G suspended mid-unwind
 * resumes its unwind correctly because CFI is driven per frame on its own
 * stack.
 *
 * Blocking (design/0005 "Runtime entry points"): channels park the G and free
 * the M on a scheduler-keyed wait queue rather than blocking the OS thread on a
 * condvar. A non-coroutine caller (the main thread) instead blocks on a
 * per-waiter condvar, which parks that OS thread without consuming a worker M.
 *
 * The netpoller (ticket #80) now parks a G blocked on a non-ready fd and frees
 * its M, re-queuing the G on readiness (see the "Netpoller" section below); it
 * reuses the `copark`/`coready` primitive here.
 *
 * DEFERRED (documented in the PR / OPEN-QUESTIONS; semantics unchanged): the
 * blocking-syscall P-handoff, preemption of long CPU loops (v1 is
 * cooperative — Gs yield only at channel ops), hardware guard pages and
 * growable/segmented stacks (a PROT_NONE guard page inside a GC_MALLOC object
 * would break conservative scanning; the spec ties real guard/growable stacks
 * to the precise-GC era, so v1 uses a fixed stack with a soft overflow check),
 * and the `sync` module / `scope:` / `select` surface still pending from #25.
 * =========================================================================== */

/* ===========================================================================
 * A1 coroutine context-switch seam (ticket #195, platform epic #158 / Area A1;
 * see design/0017-windows-coroutine-context-switch.md).
 *
 * The scheduler never calls ucontext directly: it goes through this thin seam
 * so the one load-bearing POSIX dependency (stack switching) has a single, per-
 * platform implementation.
 *
 *   zap_ctx_t                  an opaque saved execution context, stored inside
 *                              the G (and the M's scheduler context). Scanned as
 *                              heap under Boehm, exactly as the raw ucontext_t
 *                              was, so the GC invariant is unchanged.
 *   zap_ctx_make(c, stk, n, e) initialise *c to start executing e() on the n-byte
 *                              stack buffer `stk` the first time it is swapped to
 *                              (e must never return). Was getcontext+makecontext.
 *   zap_ctx_swap(from, to)     save the current context into *from and resume
 *                              *to. Was swapcontext.
 *
 * POSIX (Linux/macOS, x86_64 & aarch64): implemented as the existing ucontext
 * calls behind the seam — a ZERO-behaviour-change refactor (the seam is the
 * deliverable that unblocks Windows; swapping POSIX to asm for the per-switch
 * sigprocmask win is an optional follow-up, I#195 step 5).
 *
 * Windows (x86_64): IMPLEMENTED (I#195 / A1) as a separate asm translation unit
 * `zap_ctx_win_x64.c` (linked by build_executable's Windows arm). A `zap_ctx_t`
 * is a single saved stack pointer; zap_ctx_swap/zap_ctx_make are the hand-written
 * asm context switch from design/0017-windows-coroutine-context-switch.md — they
 * save the Windows nonvolatile GPRs (incl. RDI/RSI) + XMM6-15 and swap the TIB
 * StackBase/StackLimit/DeallocationStack on every switch. The Windows platform
 * gate at the top of this file still halts the compile before here (A3/A2/A4 are
 * unported), so the declarations below are the forward decls the scheduler calls;
 * the implementation lives in zap_ctx_win_x64.c and is exercised today, ahead of
 * a full Windows runtime, by the standalone harness zap_ctx_win_x64_test.c. */
#if defined(ZAP_PLATFORM_POSIX)
typedef ucontext_t zap_ctx_t;
static void zap_ctx_make(zap_ctx_t *c, void *stack, size_t stack_size,
                         void (*entry)(void)) {
    getcontext(c);
    c->uc_stack.ss_sp = stack;
    c->uc_stack.ss_size = stack_size;
    c->uc_link = NULL; /* bootstrap never returns */
    makecontext(c, entry, 0);
}
static inline void zap_ctx_swap(zap_ctx_t *from, zap_ctx_t *to) {
    swapcontext(from, to);
}
#else /* ZAP_PLATFORM_WINDOWS — A1 (I#195) drop-in point */
typedef struct zap_ctx_t {
    void *sp; /* single saved stack pointer; the register save-area lives on the
               * coroutine's own (runtime-owned, Boehm-scanned) green stack */
} zap_ctx_t;
void zap_ctx_make(zap_ctx_t *c, void *stack, size_t stack_size,
                  void (*entry)(void));
void zap_ctx_swap(zap_ctx_t *from, zap_ctx_t *to);
#endif

/* Fixed green-thread stack size (design/0005: 64-256 KiB fixed, no growth in
 * v1). Overridable via ZAP_STACK_SIZE (KiB) for experimentation. */
#define ZAP_STACK_DEFAULT (128 * 1024)
#define ZAP_STACK_REDZONE 8192 /* soft overflow margin checked at park points */

typedef enum { G_RUNNABLE, G_RUNNING, G_PARKED, G_DEAD } GState;

typedef struct G {
    zap_ctx_t ctx;     /* saved callee-saved regs + SP + IP (scanned as heap);
                        * via the A1 seam (ucontext on POSIX, asm on Windows)  */
    void *stack;       /* GC_MALLOC'd green stack buffer (scanned as heap)     */
    size_t stack_size;
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
    /* Preemption (ticket #172). `no_preempt` > 0 marks a window where this
     * coroutine must not be async-preempted (inside a runtime critical section
     * that holds a lock / the allocator): the Boehm signal handler only switches
     * when it is 0. It lives in the G (not a thread-local) so it follows the
     * coroutine across M migrations. `preempt_requeue` asks the scheduler to
     * requeue a preempted G on the GLOBAL queue (behind its peers) for fairness
     * rather than back onto the local P's head. */
    int no_preempt;
    int preempt_requeue;
    struct M *locked_m; /* LockOSThread (#172): dedicated M this coroutine is
                         * pinned to, or NULL. While set, the coroutine runs only
                         * on `locked_m` and is excluded from fairness preemption. */
    /* The M that most recently resumed this coroutine (I#869). `switch_to_g` stores
     * it just before swapping the coroutine in, and it is rewritten on every
     * resume, so while the coroutine is running it is the M it is running ON.
     * `g_bootstrap` reads it after the coroutine body returns to find the right
     * scheduler context to swap back into. It exists because the obvious
     * alternative — re-reading the `cur_m` thread-local after the body — is
     * miscompiled when the body parked and migrated: see the contract on `cur_m`.
     * A plain (non-GC) pointer: Ms are runtime-infrastructure memory, not heap. */
    struct M *run_m;
    int lock_depth;     /* LockOSThread nesting count (lock/unlock balance)      */
    /* Diagnostics (I#475 / D-0058). Both are static C-strings (codegen globals or
     * runtime literals), never GC-managed, so they need no tracing and stay valid
     * for the process lifetime. `name` is the spawn target; `park_reason` is what
     * this G last parked on (set at each park site), reported while it is PARKED. */
    const char *name;
    const char *park_reason;
    /* Severed orphan (#825). Set to 1 when a `scope(timeout=...)` deadline
     * detached this still-live coroutine from its scope (it did not drain within
     * the grace window) — it is reparented to nobody (Go `spawn detached`
     * accounting: scope cleared, `in_live` stays 0) and leaked until it finishes
     * on its own. The runtime never force-unwinds or frees a severed coroutine
     * (unsafe mid-FFI / mid-lock — the reason Java deprecated Thread.stop), so a
     * severed orphan's later uncaught exception is DROPPED rather than aborting
     * the process (`zap_g_uncaught`): the whole point of severing is that the
     * caller caught `ScopeTimeout` and the program keeps running. A plain
     * detached coroutine (severed == 0) keeps the Go abort-on-uncaught rule. */
    int severed;
    /* Runtime data-race detector (`--race`, #678, epic #665, D-0114). The
     * coroutine's vector-clock index, assigned lazily the first time the engine
     * sees this G (at `zap_race_on_spawn`); -1 until then and for every G in a
     * non-`--race` run (the field is written once in `galloc`, read only by the
     * engine under `race_mu`). The clock itself lives in the engine's gid-indexed
     * table, not here, so a non-`--race` build never allocates one. */
    int64_t race_gid;
} G;

typedef struct P {
    pthread_mutex_t mu; /* guards this P's local deque                         */
    G *head;            /* LIFO: push/pop at head (locality), steal at tail    */
    G *tail;
} P;

typedef struct M {
    pthread_t thread;
    zap_ctx_t sched_ctx;        /* the scheduler loop's context (native stack) */
    struct GC_stack_base native_sb; /* this M's native stack cold end          */
    G *current;                 /* G running on this M, or NULL in scheduler    */
    P *p;                       /* processor this M is attached to              */
    /* Preemption monitor (ticket #172). `schedtick` is bumped every time this M
     * picks up a coroutine; the monitor flags an M whose tick is unchanged
     * across a monitor interval (same coroutine still running) and sets
     * `preempt`, which the coroutine observes at its next safepoint (precise GC)
     * or via the async signal (Boehm). `dedicated` marks a LockOSThread worker
     * that runs a single pinned coroutine and never steals / is never flagged.
     *
     * Both are genuine cross-thread signals with no mutex ordering either side
     * (#213, D-0121): the owning M writes `schedtick`/clears `preempt`, while a
     * separate monitor thread reads `schedtick` and the SIGURG handler (another
     * thread's async context) sets `preempt`. Plain `volatile` orders nothing
     * across threads, so both are accessed ONLY via `__atomic_*` with RELAXED
     * ordering. Relaxed is provably enough: neither carries a data payload —
     * `schedtick` is a monotone liveness counter (a stale read defers one
     * fairness interval), `preempt` is an advisory "please yield" latch whose
     * GC-root safety is independent of its memory order (D-0117). `preempt` must
     * be atomic (not just volatile) because it is written from an async signal
     * handler; `__atomic` on an aligned int is async-signal-safe and lock-free. */
    uint64_t schedtick;
    int preempt;
    int dedicated;
    int ded_joinable;           /* 1 => counted in g_ded_live (LockOSThread #172);
                                 * 0 => threading-compat pin (#175), awaited via
                                 * g_live for non-daemon threads instead, so a
                                 * daemon thread never blocks program exit.      */
    G *ded_g;                   /* the single pinned coroutine (dedicated M)    */
    pthread_mutex_t ded_mu;     /* guards ded_runnable / ded_stop               */
    pthread_cond_t ded_cv;
    volatile int ded_runnable;  /* the pinned coroutine is ready to run         */
    volatile int ded_stop;      /* tear the dedicated M down                    */
#if defined(ZAP_PLATFORM_WINDOWS)
    /* Preemption on Windows (I#198 wiring of the A3/#197 primitive): the monitor
     * has no SIGURG to deliver, so it HIJACKS this worker (SuspendThread +
     * SetThreadContext) via zap_win_preempt_try. `win_thread` is a real,
     * full-access duplicate of the worker's thread handle (captured in
     * worker_main); `in_hijack` is the per-M in-flight guard the primitive
     * check-and-sets and copreempt clears, so a second hijack can never nest on
     * the green stack (ticket #197 Q5). Windows-only #ifdef: the Linux/macOS M
     * layout is byte-for-byte unchanged. */
    void *win_thread;
    volatile LONG in_hijack;
#endif
} M;

/* Per-waiter node on a channel's sender/receiver queue. A coroutine waiter
 * (`g != NULL`) is re-enqueued on the scheduler when woken; a native-thread
 * waiter blocks on its own condvar. The waker fills `val`/`ok`/`closed` under
 * the channel lock *before* waking, so the woken waiter reads its outcome
 * without re-locking. */
struct SelState; /* fwd: set on a waiter that belongs to a `select` (D-0031) */
typedef struct Waiter {
    struct Waiter *next;
    G *g;                 /* NULL => native-thread (e.g. main) waiter          */
    pthread_cond_t cv;    /* native waiter only                                */
    volatile int done;    /* native waiter: set by the waker                   */
    uint64_t val;         /* send: value offered; recv: value delivered        */
    int ok;               /* recv: 1 iff a real value was delivered            */
    int closed;           /* set iff the channel closed while this waiter slept */
    /* `select` (D-0031, #1002): when non-NULL this waiter is one case of a
     * `select`; a counterpart matching it must CLAIM the select (CAS its
     * single-fire flag under `sel->mu`) before completing, and skip it if the
     * select already fired on another channel. NULL (the `{0}` default at every
     * plain send/recv site) means an ordinary single-op waiter. */
    struct SelState *sel;
    int sel_case;         /* which select case index this waiter represents */
} Waiter;

/* ---- scheduler globals ---------------------------------------------------- */
/* The M running this thread; NULL on main.
 *
 * CONTRACT (I#869, D-0160; enforced for every site by I#891): a thread-local's
 * ADDRESS must never be held across a point where the running coroutine can park.
 * A coroutine is resumed on whichever worker thread picks it up, so after any
 * `zap_ctx_swap` / `copark` / `cosched` / call into code that can block, "this
 * thread" is a different OS thread. On aarch64 at -O2 the compiler materialises a
 * thread-local's address (`mrs tpidr_el0` + offset) ONCE per function and keeps it
 * in a callee-saved register, which `swapcontext` restores unchanged on resume: a
 * post-park access then reads/writes the OLD thread's slot. (x86_64 executables
 * address TLS as `%fs:off` on every access, so they do not show it; an x86_64
 * -fPIC build can.) `g_bootstrap` hit it first (I#869), then the no-preempt
 * bookkeeping: `chan_block` parks and afterwards runs the inlined
 * `zap_np_lock/unlock`, which bumped the depth of whatever coroutine was running
 * on the OLD thread (I#891) and left the real one preemptible inside a lock or
 * inside libgc.
 *
 * So the storage is private and EVERY access goes through `zap_cur_m_slot()`: a
 * `noinline` function whose volatile asm keeps both GCC's and LLVM's IPA from
 * inferring it const/pure and merging calls, so the thread pointer is recomputed on
 * each use and can never be cached across a park. `cur_m` is a macro for the
 * dereferenced slot, so reads, writes and `cur_m->field` all read naturally. The
 * coroutine-side way to find its M after resuming is still `g->run_m`
 * (`switch_to_g`). */
static __thread M *zap_cur_m_tls = NULL;
static __attribute__((noinline)) M **zap_cur_m_slot(void) {
    __asm__ __volatile__("" ::: "memory");
    return &zap_cur_m_tls;
}
#define cur_m (*zap_cur_m_slot())

static M *g_ms = NULL;
static P *g_ps = NULL;
static int g_nm = 0;             /* number of Ms / Ps                          */

static pthread_mutex_t g_global_mu = PTHREAD_MUTEX_INITIALIZER; /* global runq */
static G *g_global_head = NULL;
static G *g_global_tail = NULL;

static pthread_mutex_t g_idle_mu = PTHREAD_MUTEX_INITIALIZER; /* idle-M parking */
static pthread_cond_t g_idle_cv = PTHREAD_COND_INITIALIZER;
/* One-shot exit latch (#213, D-0121). The scheduler reads it under `g_idle_mu`
 * (paired with the broadcast at shutdown, so parked worker Ms are fully ordered);
 * the preemption monitor also polls it WITHOUT any lock. `volatile` gives that
 * unlocked poll no well-defined cross-thread load, so the flag is accessed via
 * `__atomic_*` with RELAXED ordering. Relaxed suffices: it carries no payload
 * (a pure "time to exit" trigger), and real liveness/ordering come from the
 * `g_idle_cv` broadcast + 1 ms timed re-poll on the scheduler side and the
 * monitor thread join on the shutdown side. */
static int g_shutdown = 0;

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

/* ---- preemption no-preempt guard (ticket #172) ---------------------------- */
/* Raise/lower the running coroutine's no-preempt depth. The count lives in the G
 * so it follows the coroutine across M migrations; the Boehm async-preemption
 * handler only switches when it is 0. A no-op on a native thread (main) or in
 * the scheduler loop (no current coroutine), which are never async-preempted. */
static __attribute__((noinline)) int *zap_np_slot(void) {
    __asm__ __volatile__("" ::: "memory"); /* never const/pure: not mergeable across a park (I#891) */
    M *m = zap_cur_m_tls;                  /* fresh thread-local read on EVERY call */
    return (m && m->current) ? &m->current->no_preempt : NULL;
}
static void zap_np_enter(void) { zap_np_begin(zap_np_slot()); }
static void zap_np_leave(void) { zap_np_end(zap_np_slot()); }
/* Guarded lock/unlock for the runtime critical sections a long, non-parking
 * compute loop can touch (channel fast paths, sync primitives): holding one of
 * these mutexes across an async preemption could deadlock a switched-in sibling
 * on the same M under Boehm. Pairs with the per-coroutine no-preempt depth; the
 * park primitives clear the depth when a coroutine yields with a lock held. */
static void zap_np_lock(pthread_mutex_t *mu) {
    zap_np_enter();
    pthread_mutex_lock(mu);
}
static void zap_np_unlock(pthread_mutex_t *mu) {
    pthread_mutex_unlock(mu);
    zap_np_leave();
}

/* ---- GC stack scanning across context switches (I#885) -------------------- */
/* Under Boehm a stopped thread is scanned as `[sp, stack_end)`: `sp` is captured
 * by the stop-the-world signal handler at whatever instruction the thread was on,
 * and `stack_end` is the thread's registered stack cold end. An M thread runs on
 * TWO kinds of stack — its native (scheduler) stack, and a coroutine's green stack
 * (a GC_MALLOC buffer) — so while a coroutine runs `sp` is on the green stack and
 * a `stack_end` at the native top would make `[sp, stack_end)` a wild range
 * spanning unrelated mappings (a mark thread faults in the unmapped holes).
 *
 * The scheduler used to follow the switch by moving `stack_end` (`set_bottom`):
 * to the green stack top before the swap, back to the native top after. But `sp`
 * and `stack_end` could not be changed atomically — `stack_end` moved before the
 * swap that moves `sp`, and back only after swapping back — so a stop-the-world
 * landing in either gap (a switch-heavy program is in one a large fraction of the
 * time; the swap itself is a syscall) saw `sp` on one stack and `stack_end` on the
 * other and crashed the collector. (#885; the old comment here argued either
 * value of `stack_end` was "valid" — true only when it is consistent with `sp`.)
 *
 * Now `stack_end` is NEVER moved: it stays the native stack top for the thread's
 * whole life, so the native case is always right and the switch path calls nothing
 * in the collector (which also keeps it lock-free, #223 / D-0117). The running
 * coroutine case is fixed up where it is safe — when the collector pushes the
 * stacks, with the world stopped and every thread's `sp` already captured — by
 * bdwgc's `GC_set_sp_corrector` hook (documented for "a crude integration with
 * certain coroutine implementations"): a thread whose captured `sp` lies inside a
 * coroutine's stack buffer is mapped to an EMPTY range `[native_top, native_top)`.
 * Nothing is lost: a green stack is itself a GC_MALLOC object kept reachable
 * through its `G` (registered in `g_all`), so it is conservatively scanned in full
 * as heap — including the register snapshot / signal frame the stop-the-world
 * handler pushes on it, and the collecting thread's own spilled registers — and
 * the scheduler's native frames, which were not scanned while a G ran before
 * either, hold no GC pointers that are not also reachable from the `G`s. Under
 * gc-immix there is no thread-stack scanning (precise roots), so this is Boehm
 * only. The hook runs with the GC lock held and possibly the world stopped: it
 * only reads `g_all` / `G` / `M` fields and takes no lock and allocates nothing. */
#if defined(ZAP_PLATFORM_POSIX) && !defined(ZAP_GC_IMMIX)
static void zap_sp_corrector(void **sp_ptr, void *pthread_id) {
    (void)pthread_id;
    char *sp = (char *)*sp_ptr;
    /* Lock-free read of the registry: the world is stopped (or this is the
     * collecting thread itself), so no entry is being rewritten; a stopped thread
     * frozen mid-`all_add`/`all_remove` leaves every index below `g_all_len`
     * pointing at a live (or NULL) G. */
    G **all = g_all;
    int64_t n = g_all_len;
    for (int64_t i = 0; i < n; i++) {
        G *g = all[i];
        if (g == NULL) {
            continue;
        }
        char *lo = (char *)g->stack;
        if (sp >= lo && sp < lo + g->stack_size) {
            /* The thread is running this coroutine: report an empty native range.
             * `run_m` is the M that resumed it (switch_to_g), i.e. this thread. */
            M *m = g->run_m;
            if (m != NULL && m->native_sb.mem_base != NULL) {
                *sp_ptr = m->native_sb.mem_base;
            }
            return;
        }
    }
}
#endif

/* ---- run queues ----------------------------------------------------------- */
static void global_push(G *g) {
    zap_np_lock(&g_global_mu);
    g->link = NULL;
    if (g_global_tail) {
        g_global_tail->link = g;
    } else {
        g_global_head = g;
    }
    g_global_tail = g;
    zap_np_unlock(&g_global_mu);
}
static G *global_pop(void) {
    zap_np_lock(&g_global_mu);
    G *g = g_global_head;
    if (g) {
        g_global_head = g->link;
        if (!g_global_head) {
            g_global_tail = NULL;
        }
        g->link = NULL;
    }
    zap_np_unlock(&g_global_mu);
    return g;
}
static void p_push(P *p, G *g) {
    zap_np_lock(&p->mu);
    g->link = p->head;
    p->head = g;
    if (!p->tail) {
        p->tail = g;
    }
    zap_np_unlock(&p->mu);
}
static G *p_pop(P *p) { /* LIFO from head (owner) */
    zap_np_lock(&p->mu);
    G *g = p->head;
    if (g) {
        p->head = g->link;
        if (!p->head) {
            p->tail = NULL;
        }
        g->link = NULL;
    }
    zap_np_unlock(&p->mu);
    return g;
}
static G *p_steal(P *p) { /* FIFO from tail (thief) */
    zap_np_lock(&p->mu);
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
    zap_np_unlock(&p->mu);
    return g;
}

/* Make `g` runnable: route a LockOSThread-pinned coroutine to its dedicated M;
 * a coroutine yielded by preemption to the GLOBAL queue (behind its peers, for
 * fairness); otherwise push to the current M's P (locality) or the global queue,
 * then wake one idle M. */
static void sched_enqueue(G *g) {
    g->state = G_RUNNABLE;
    /* LockOSThread (#172): a pinned coroutine only ever runs on its dedicated M.
     * Hand it to that M's private slot and wake it, bypassing the shared pool. */
    if (g->locked_m) {
        M *d = g->locked_m;
        zap_np_lock(&d->ded_mu);
        d->ded_runnable = 1;
        pthread_cond_signal(&d->ded_cv);
        zap_np_unlock(&d->ded_mu);
        return;
    }
    if (g->preempt_requeue) {
        /* A preempted coroutine goes to the back of the global queue so its peers
         * (already queued ahead) get the M first — otherwise a local-head requeue
         * would re-pick the same coroutine immediately and defeat preemption. */
        g->preempt_requeue = 0;
        global_push(g);
    } else if (cur_m && cur_m->p) {
        p_push(cur_m->p, g);
    } else {
        global_push(g);
    }
    zap_np_lock(&g_idle_mu);
    pthread_cond_signal(&g_idle_cv);
    zap_np_unlock(&g_idle_mu);
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
        zap_np_lock(&g_idle_mu);
        if (__atomic_load_n(&g_shutdown, __ATOMIC_RELAXED)) {
            zap_np_unlock(&g_idle_mu);
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
        zap_np_unlock(&g_idle_mu);
    }
}

/* Scheduler-side switch into `g`. (The collector's view of this M's stack is not
 * touched here: `stack_end` is fixed and a thread captured on a green stack is
 * corrected at scan time by `zap_sp_corrector`, I#885.) On return the G has
 * either parked (unlock the channel mutex it asked us to release, completing the
 * park/wake handoff) or finished (reclaim it). */
static void gfree(G *g);
static void sched_gdone(void);
static void scope_gdone(struct Scope *sc);
static void sched_enqueue(G *g);
static void switch_to_g(M *m, G *g) {
    m->current = g;
    /* I#869: tell the coroutine which M is resuming it NOW. A coroutine can park
     * on one OS thread and resume on another, so this is rewritten on every
     * switch-in; `g_bootstrap` uses it to swap back into the right scheduler
     * context when the coroutine's body returns (it must not re-read `cur_m`). */
    g->run_m = m;
    g->state = G_RUNNING;
    /* Preemption monitor (#172): a fresh scheduling tick + a clean preempt flag.
     * The monitor samples `schedtick`; an unchanged tick across an interval means
     * the same coroutine monopolized this M and earns a preemption request. */
    __atomic_fetch_add(&m->schedtick, 1, __ATOMIC_RELAXED);
    __atomic_store_n(&m->preempt, 0, __ATOMIC_RELAXED);
#ifdef ZAP_GC_IMMIX
    /* Restore this coroutine's shadow-stack head into the per-thread cell the
     * compiled code mutates; save it back when the G switches out (parks /
     * yields / dies). A running G's head thus lives in zap_shadow_head and is
     * flushed to g->shadow_head at a safepoint or here. */
    zap_shadow_head = g->shadow_head;
#endif
    zap_ctx_swap(&m->sched_ctx, &g->ctx);
    /* I#889: the coroutine is stopped (it published its state and swapped out), so
     * end the no-preempt window it opened in `g_enter_switch_out`. */
    g->no_preempt = 0;
#ifdef ZAP_GC_IMMIX
    g->shadow_head = zap_shadow_head;
#endif
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
         * releases the program-exit join; a detached G is joined by nobody.
         *
         * #825: atomically CLAIM the scope's live-count decrement by swapping
         * `g->scope` to NULL. A concurrent `scope_sever_live` (a timed-out
         * deadline severing a non-draining sibling) claims the same way, so
         * whichever runs first owns the single `scope_gdone` and the other sees
         * NULL and skips it — the owned count is never decremented twice. */
        struct Scope *sc =
            __atomic_exchange_n(&g->scope, (struct Scope *)NULL, __ATOMIC_ACQ_REL);
        int in_live = g->in_live;
        gfree(g);
        if (sc) {
            scope_gdone(sc);
        } else if (in_live) {
            sched_gdone();
        }
    } else if (g->state == G_RUNNABLE) {
        /* The G yielded (`cosched`, e.g. at `checkpoint()`): requeue it. */
        sched_enqueue(g);
    }
}

static void *g_entry_fn(G *g); /* fwd */

/* I#889: the publish-state -> swap span is a no-preempt window.
 *
 * A coroutine that is about to switch away tells the scheduler WHY by storing
 * `g->state` (G_DEAD / G_PARKED / G_RUNNABLE) and then calling `zap_ctx_swap`;
 * `switch_to_g` dispatches on that state when the swap returns. If the Boehm
 * async-preemption handler (SIGURG) fires between the store and the actual
 * switch — the swap is a PLT stub, glibc's context save and an `rt_sigprocmask`
 * syscall, so that is microseconds, not instructions — `copreempt` overwrites the
 * state with G_RUNNABLE and switches away; when the scheduler later resumes the
 * coroutine inside the handler it sets G_RUNNING, the handler returns, and the
 * interrupted park/exit sequence performs ITS swap with state == G_RUNNING, which
 * `switch_to_g` has no branch for: an exiting coroutine is never reclaimed
 * (`g_live` never reaches 0 -> hang at join), a parking one never has its
 * `parkmu` unlocked nor is woken.
 *
 * So the span is made non-preemptible: raise the depth BEFORE the state store
 * (the compiler barrier makes the handler, which runs on this very thread, observe
 * the stores in that order), do NOT clear it on the coroutine side, and let
 * `switch_to_g` clear it once the coroutine is stopped. The depth is per-G, so it
 * cannot leak to a sibling. (`copark`/`cosched` used to clear it BEFORE the swap.) */
static inline void g_enter_switch_out(G *g) {
    g->no_preempt = 1;
    __atomic_signal_fence(__ATOMIC_SEQ_CST);
}

/* The makecontext bootstrap: runs on the green stack. Re-reads `cur_m` after
 * the call returns because a G may be resumed on a different M than it started
 * on. Never returns — switches back to whatever M's scheduler is running it. */
static void g_bootstrap(void) {
    M *m = cur_m;
    G *g = m->current;
    g_entry_fn(g);
    /* The body may have parked and been resumed on a DIFFERENT M (OS thread), so
     * `m` from above can be stale. Do NOT re-read `cur_m` here (I#869): it is a
     * `__thread` variable, and on aarch64 at -O2 the compiler computes its address
     * (`mrs tpidr_el0` + offset) once and holds it in a callee-saved register
     * across the whole body; `swapcontext` restores that register unchanged on
     * resume, so the "re-read" would load the OLD thread's slot and swap into a
     * scheduler context another thread is still running on (SIGSEGV / stack
     * smashing). `g->run_m` is the M that resumed this coroutine most recently
     * (set by `switch_to_g`), read from the heap-allocated G, which the compiler
     * cannot cache across the opaque body call. */
    m = g->run_m;
    g_enter_switch_out(g); /* I#889: no preemption between the state store and the swap */
    g->state = G_DEAD;
    zap_ctx_swap(&g->ctx, &m->sched_ctx);
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
#if defined(ZAP_PLATFORM_WINDOWS)
    /* Preemption on Windows (#172 via the A3 hijack): the monitor needs a real,
     * full-access handle to this worker thread to SuspendThread/SetThreadContext
     * it. GetCurrentThread() is a pseudo-handle usable only by this thread, so
     * duplicate it into a real one the monitor can use. Also give this thread the
     * fault handler's stack headroom (the sigaltstack analogue; A3). */
    {
        HANDLE real = NULL;
        if (DuplicateHandle(GetCurrentProcess(), GetCurrentThread(),
                            GetCurrentProcess(), &real,
                            THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT |
                                THREAD_SET_CONTEXT,
                            FALSE, 0)) {
            m->win_thread = real;
        }
        m->in_hijack = 0;
        zap_win_fault_thread_init();
    }
#else
    /* Ensure the async-preemption signal is deliverable to this worker (#172):
     * unblock SIGURG, which the monitor targets with pthread_kill. */
    {
        sigset_t s;
        sigemptyset(&s);
        sigaddset(&s, SIGURG);
        pthread_sigmask(SIG_UNBLOCK, &s, NULL);
    }
#endif
    /* This thread was created by GC_pthread_create, so it is registered; record
     * its GC handle + native stack bottom for the switch bracketing. */
    struct GC_stack_base sb;
    (void)GC_get_my_stackbottom(&sb);
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
    zap_np_lock(&g_all_mu);
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
    zap_np_unlock(&g_all_mu);
}
static void all_remove(G *g) {
    zap_np_lock(&g_all_mu);
    int64_t i = g->all_idx;
    if (i >= 0 && i < g_all_len && g_all[i] == g) {
        G *last = g_all[--g_all_len];
        g_all[i] = last;
        last->all_idx = i;
        g_all[g_all_len] = NULL;
    }
    zap_np_unlock(&g_all_mu);
}

/* Runtime data-race detector hooks called from the scheduler / sync primitives
 * (`--race`, #678, epic #665, D-0114). The engine itself is defined in
 * zaprace.inc.c (included near the end of this file); these forward declarations
 * let the earlier `zap_spawn` / `zap_mutex_*` call sites reference it. Each is a
 * no-op until the detector is armed (by `__zap_race_enable` /
 * `__zap_race_register_global` / an access hook — all emitted only under
 * `--race`), so a default build is behaviourally unchanged. `zap_race_on_spawn`
 * records the
 * parent->child happens-before edge; `zap_race_mutex_{acquire,release}` carry
 * the lock acquire/release edges keyed by the mutex pointer. */
void zap_race_on_spawn(G *child);
void zap_race_mutex_acquire(void *mu);
void zap_race_mutex_release(void *mu);
/* Shadow-table relocation hook (slice 4, #684): the precise Immix evacuator
 * (`zap_evac_copy` in zapgc.inc.c, #included BEFORE zaprace.inc.c) calls this for
 * every object it moves so a heap location's shadow follows the object through
 * the copy. A no-op until the detector is armed, and never called by the
 * non-moving Boehm backend. */
void zap_race_on_move(void *old_payload, void *new_payload, int64_t size);
/* Slice-3 (#679) happens-before edges. `chan_xfer` is a rendezvous hand-off
 * (sender `G` -> receiver `G`); `chan_buf_store`/`chan_buf_load` split a buffered
 * hand-off across a ring-buffer slot address; the `wg_*`/`once_*` pairs carry the
 * WaitGroup and Once completion edges keyed by the handle pointer. All inert
 * until the engine is armed, so a non-`--race` build is unchanged. */
void zap_race_chan_xfer(void *from_g, void *to_g);
void zap_race_chan_buf_store(void *slot, void *from_g);
void zap_race_chan_buf_load(void *slot, void *to_g);
void zap_race_wg_release(void *wg);
void zap_race_wg_acquire(void *wg);
void zap_race_once_release(void *once);
void zap_race_once_acquire(void *once);

static G *galloc(void *fn, void *arg) {
    size_t ss = zap_stack_size();
    G *g = NULL;
    zap_np_lock(&g_free_mu);
    if (g_free_head) {
        g = g_free_head;
        g_free_head = g->link;
    }
    zap_np_unlock(&g_free_mu);
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
    g->run_m = NULL; /* I#869: set by switch_to_g on every resume */
    g->no_preempt = 0; /* I#889: a recycled G must not inherit a stale window */
    g->scope = NULL;
    g->in_live = 0;
    g->name = NULL;
    g->park_reason = NULL;
    g->severed = 0; /* #825: not a timed-out orphan until a deadline severs it */
    g->shadow_head = NULL; /* precise GC (#48): a fresh G has an empty chain */
    g->race_gid = -1;      /* unassigned until the race engine first sees it */
    zap_ctx_make(&g->ctx, g->stack, g->stack_size, g_bootstrap);
    all_add(g);
    return g;
}

static void gfree(G *g) {
    all_remove(g);
    g->fn = NULL;
    g->arg = NULL;
    g->scope = NULL; /* drop the scope reference so a reused G starts clean */
    g->in_live = 0;
    g->severed = 0; /* #825: a reused G is not a severed orphan */
    /* Keep the stack buffer for reuse (size-classed free list, one class). */
    zap_np_lock(&g_free_mu);
    g->link = g_free_head;
    g_free_head = g;
    zap_np_unlock(&g_free_mu);
}

static void sched_gdone(void) {
    zap_np_lock(&g_live_mu);
    if (--g_live == 0) {
        pthread_cond_broadcast(&g_live_cv);
    }
    zap_np_unlock(&g_live_mu);
}

/* ---- scheduler init (lazy, on first spawn) -------------------------------- */
static pthread_once_t g_sched_once = PTHREAD_ONCE_INIT;

/* Container-aware CPU budget (#905, D-0176). Under a Kubernetes CFS CPU quota
 * (`resources.limits.cpu`) the cgroup limits CPU *time*, not *which cores*, so
 * `sysconf(_SC_NPROCESSORS_ONLN)` (and `nproc`, `/proc/cpuinfo`, even the
 * affinity mask) all report the NODE's core count, not the pod's budget — a Zap
 * service in a `cpu: 1000m` pod on a 16-core node would otherwise spin 16 worker
 * Ms for a 1-core budget (and any BLAS hand-off would over-thread the same way).
 * The effective budget is `min(online, affinity, ceil(cfs_quota/period))`.
 * These readers are file-based (no `_GNU_SOURCE` / `sched_getaffinity`, so the
 * portable / no-CPython and Windows builds still compile); a missing or
 * unlimited source returns -1 (ignored in the min). */

/* ceil(quota/period) as a CPU count, or -1 for unlimited / invalid. */
static long zap_cpu_quota_ncpu(long quota, long period) {
    if (quota <= 0 || period <= 0) {
        return -1; /* v1 quota == -1 (and 'max' in v2) map here: unlimited */
    }
    long n = (quota + period - 1) / period;
    return n < 1 ? 1 : n;
}

/* Parse a cgroup v2 `cpu.max` line ("<quota> <period>" or "max [period]") into
 * a CPU count, or -1 if unlimited / unparseable. */
static long zap_parse_cpu_max(const char *s) {
    while (*s == ' ' || *s == '\t') {
        s++;
    }
    if (strncmp(s, "max", 3) == 0) {
        return -1;
    }
    long q = 0, p = 0;
    if (sscanf(s, "%ld %ld", &q, &p) != 2) {
        return -1;
    }
    return zap_cpu_quota_ncpu(q, p);
}

/* First line of `path` into `buf`; 1 on success, 0 if unreadable. */
static int zap_read_first_line(const char *path, char *buf, size_t n) {
    FILE *f = fopen(path, "r");
    if (!f) {
        return 0;
    }
    char *r = fgets(buf, (int)n, f);
    fclose(f);
    return r != NULL;
}

/* The cgroup CPU quota as a CPU count, or -1 if unlimited / unknown. cgroup v2
 * (`cpu.max`) then v1 (`cpu.cfs_quota_us` / `cpu.cfs_period_us`). `ZAP_CPU_MAX`
 * overrides the v2 read (a v2-format string, e.g. "150000 100000" or "max") —
 * a diagnostic / testing seam and an escape hatch for non-standard mounts. */
static long zap_cgroup_ncpu(void) {
    const char *env = getenv("ZAP_CPU_MAX");
    if (env && *env) {
        return zap_parse_cpu_max(env);
    }
#if defined(__linux__)
    char buf[128];
    if (zap_read_first_line("/sys/fs/cgroup/cpu.max", buf, sizeof buf)) {
        return zap_parse_cpu_max(buf); /* cgroup v2 */
    }
    char qb[64], pb[64];
    if (zap_read_first_line("/sys/fs/cgroup/cpu,cpuacct/cpu.cfs_quota_us", qb, sizeof qb) &&
        zap_read_first_line("/sys/fs/cgroup/cpu,cpuacct/cpu.cfs_period_us", pb, sizeof pb)) {
        return zap_cpu_quota_ncpu(atol(qb), atol(pb)); /* cgroup v1 */
    }
#endif
    return -1;
}

/* The CPU affinity count from `/proc/self/status` `Cpus_allowed_list`
 * (a comma-separated list of single CPUs and `a-b` ranges, the cpuset case), or
 * -1 if unavailable. File-based so it needs no `_GNU_SOURCE`. */
static long zap_affinity_ncpu(void) {
#if defined(__linux__)
    FILE *f = fopen("/proc/self/status", "r");
    if (!f) {
        return -1;
    }
    char line[512];
    long count = -1;
    while (fgets(line, sizeof line, f)) {
        if (strncmp(line, "Cpus_allowed_list:", 18) != 0) {
            continue;
        }
        count = 0;
        char *p = line + 18;
        while (*p) {
            while (*p == ' ' || *p == '\t') {
                p++;
            }
            if (*p < '0' || *p > '9') {
                p++;
                continue;
            }
            long a = strtol(p, &p, 10);
            long b = a;
            if (*p == '-') {
                p++;
                b = strtol(p, &p, 10);
            }
            if (b >= a) {
                count += (b - a + 1);
            }
            while (*p && *p != ',') {
                p++;
            }
            if (*p == ',') {
                p++;
            }
        }
        break;
    }
    fclose(f);
    return count;
#else
    return -1;
#endif
}

/* The parallelism knob (ticket #172): env var `MAXPROCS` sets the number of
 * worker Ms (== Ps). Dropped the `ZAP_` prefix and the GOMAXPROCS-analogue
 * naming — it is simply `MAXPROCS`, our term. `zap_runtime_maxprocs()` exposes
 * the effective value to Zap. Default: the online hardware-thread count, clamped
 * DOWN to a cgroup CFS quota / cpuset affinity when present (container-aware,
 * #905 / D-0176) so a Zap service does not over-thread under a k8s CPU limit. */
static int zap_maxprocs(void) {
    const char *e = getenv("MAXPROCS");
    if (e) {
        long n = atol(e);
        if (n >= 1 && n <= 1024) {
            return (int)n;
        }
    }
#if defined(ZAP_PLATFORM_WINDOWS)
    SYSTEM_INFO si;
    GetSystemInfo(&si);
    long eff = (long)si.dwNumberOfProcessors;
#else
    long eff = sysconf(_SC_NPROCESSORS_ONLN);
    if (eff < 1) {
        eff = 1;
    }
    long quota = zap_cgroup_ncpu();
    if (quota >= 1 && quota < eff) {
        eff = quota;
    }
    long aff = zap_affinity_ncpu();
    if (aff >= 1 && aff < eff) {
        eff = aff;
    }
#endif
    if (eff < 1) {
        eff = 1;
    }
    if (eff > 256) {
        eff = 256;
    }
    return (int)eff;
}

/* ===========================================================================
 * Async (signal-based) preemption — Go 1.14 model (ticket #172, D-0008/0005)
 * ---------------------------------------------------------------------------
 * v1 was cooperative: a tight, allocation-free, yield-free CPU loop could
 * monopolize its M, starve peer coroutines, and (under the precise collector)
 * stall a pending stop-the-world GC. Preemption closes that gap WITHOUT a
 * per-iteration codegen poll (the rejected "back-edge polling" option):
 *
 *   - A dedicated MONITOR thread samples each worker M's `schedtick` every
 *     ~10 ms. An M whose tick is unchanged across an interval has run the SAME
 *     coroutine for the whole slice (any park/yield bumps the tick via
 *     switch_to_g); the monitor sets `m->preempt` and sends it SIGURG.
 *   - The SIGURG handler runs ON the target worker thread. How it yields reuses
 *     the collector's own safe-point machinery and so splits by collector:
 *       * PRECISE (gc-immix): the handler only records the request; the yield
 *         happens at the next `zap_safepoint()` (already emitted at function
 *         entry and loop back-edges for the GC — FORK 4a), where the shadow
 *         chain is consistent and parking is provably GC-safe. This is exactly
 *         the "reuse the precise-GC safepoint/stackmap machinery" the design
 *         calls for; a tight compiled loop reaches a safepoint within one
 *         iteration.
 *       * CONSERVATIVE (gc-boehm): there is no cooperative safepoint poll, but
 *         the collector scans every coroutine's saved context + green stack
 *         conservatively, so a coroutine preempted at an arbitrary instruction
 *         is still GC-safe. The handler therefore switches immediately
 *         (`copreempt`), provided the coroutine holds no runtime lock / is not
 *         mid-allocation (`no_preempt == 0`); otherwise it defers and the
 *         monitor retries on the next interval. The `no_preempt` guard covers
 *         the one lock a non-parking compute loop can touch (the allocator) plus
 *         channel/sync fast paths, so a switched-in sibling never deadlocks on a
 *         lock the preempted coroutine still holds.
 *   - A preempted coroutine is requeued on the GLOBAL run queue (`copreempt`),
 *     behind its peers, so it cannot immediately re-grab its M.
 *
 * LockOSThread-pinned coroutines (below) run on a dedicated M and are never
 * flagged (the monitor skips `dedicated` Ms) — they own their thread outright.
 * The subsystem is lazy: the monitor starts with the scheduler (first spawn), so
 * non-concurrent programs pay nothing.
 * =========================================================================== */
#define ZAP_PREEMPT_SLICE_NS 10000000L /* ~10 ms time slice before preemption   */
static pthread_t g_monitor_thread;
static int g_monitor_started = 0;
static void copreempt(void); /* fwd (scheduler yield, defined below)            */

#if defined(ZAP_PLATFORM_WINDOWS)
/* Serializes the monitor's SuspendThread hijack against bdwgc's own
 * stop-the-world SuspendThread (ticket #197 Q5): the monitor holds it across a
 * preempt attempt, and the Boehm STW event bracket (below) holds it PRE..POST, so
 * the two never suspend a worker concurrently. A real mutex, not the racy flag
 * the POSIX path uses — Windows has no signal-mask serialization. */
static pthread_mutex_t g_preempt_stw_mu = PTHREAD_MUTEX_INITIALIZER;
#endif

#ifndef ZAP_GC_IMMIX
/* Boehm stop-the-world guard (ticket #172; ordering hardened for #693/#213,
 * D-0120). Boehm's STW suspends threads with a signal and expects a suspended
 * thread to stay put; an async-preemption switch fired on a thread Boehm is
 * stopping would move it out from under the collector. `GC_set_on_collection_event`
 * brackets STW: the flag is set BEFORE any thread is suspended (PRE_STOP_WORLD) and
 * cleared only after all resume (POST_START_WORLD), so a signal handler that
 * re-reads it stays out of a collection.
 *
 * The flag is accessed ONLY through `__atomic` with seq-cst ordering (never plain
 * `volatile`, which the prior form used). `volatile` orders nothing across threads:
 * on a weak-memory arch (aarch64, #213) the collector's store here and a worker's
 * load in the preempt/prof handler have no happens-before, so the handler could
 * observe a stale 0 after STW had already begun and switch a thread out from under
 * the collector — the memory-ordering half of the #693 TOCTOU. The release store
 * below, paired with the acquire loads in `zap_preempt_handler` / the SIGPROF
 * sampler, closes that half: once PRE_STOP_WORLD returns (before any suspend), a
 * later handler load is guaranteed to see 1. The check->act residual (STW begins
 * AFTER a handler's load but BEFORE its swap finishes) is left as a benign
 * fairness window, not a correctness lock — see the handler for why that residual
 * is GC-root-safe (D-0117 invariant) and why POSIX needs no Windows-style mutex.
 *
 * On Windows the preempt path is a monitor-side thread hijack (no in-thread signal
 * handler to re-read a flag), so the bracket ALSO acquires/releases
 * `g_preempt_stw_mu` — the monitor holds that same mutex across a hijack, which is
 * the real serialization against bdwgc's SuspendThread-based STW (Q5). */
static sig_atomic_t g_boehm_in_stw = 0; /* read/written only via __atomic seq-cst */
static void zap_boehm_gc_event(GC_EventType e) {
    if (e == GC_EVENT_PRE_STOP_WORLD) {
#if defined(ZAP_PLATFORM_WINDOWS)
        pthread_mutex_lock(&g_preempt_stw_mu);
#endif
        __atomic_store_n(&g_boehm_in_stw, 1, __ATOMIC_SEQ_CST);
    } else if (e == GC_EVENT_POST_START_WORLD) {
        __atomic_store_n(&g_boehm_in_stw, 0, __ATOMIC_SEQ_CST);
#if defined(ZAP_PLATFORM_WINDOWS)
        pthread_mutex_unlock(&g_preempt_stw_mu);
#endif
    }
}
#endif
static void cosched(void);   /* fwd (cooperative yield, defined below)          */
void zap_sched_init(int64_t n_threads); /* fwd (defined below)                  */

/* Coalesced safepoint-poll word (#860, D-0155). The inlined back-edge poll (#568,
 * D-0144) used to load THREE separate `volatile int` globals every iteration —
 * `zap_gc_wanted`, `zap_collecting` (both in zapgc.inc.c) and `zap_preempt_any`
 * (here) — OR them, and branch to the out-of-line `zap_safepoint` slow path when
 * any was set. On `loop_sum`'s 30M-iteration tight loop that is 90M loads of
 * globals that are almost always zero (#857 register-promotes the loop locals, so
 * these three poll loads became the visible remaining per-iteration cost). They
 * are now three BIT LANES of ONE `volatile uint32_t`, so the poll is a single
 * `load volatile i32` + `icmp ne 0` + branch (see `Cg::gc_safepoint`).
 *
 * Placement, frequency, volatile semantics (so #857's `-O2` still cannot hoist
 * the load out of the loop) and the opaque `zap_safepoint` slow path are ALL
 * unchanged — only the flag ENCODING changed. A coalesced-word false positive
 * (any lane set) is the same harmless extra slow-path visit as before: the slow
 * path re-checks the precise per-M preempt latch and the GC handshake.
 *
 * The three lanes have three INDEPENDENT writers — the allocator sets GC_WANTED
 * (zapgc.inc.c), the collector sets/clears COLLECTING and clears GC_WANTED, and
 * the fairness monitor republishes PREEMPT every ~10 ms tick — so a writer must
 * not clobber another writer's lane. Set/clear are therefore ATOMIC bit RMW
 * (`__atomic_fetch_or`/`__atomic_fetch_and`), never a plain store that would
 * load-modify-store the whole word. RELAXED ordering suffices: these are advisory
 * flags whose slow path re-checks the precise state under `zap_stw_mu`, exactly
 * as the old separate flags did (`m->preempt` is itself a RELAXED latch, #213 /
 * D-0121). Reads are a RELAXED atomic load of the whole word masked to the lane —
 * a single aligned 32-bit load, the same cost as the old per-flag read.
 *
 * Exported (external linkage) so codegen's inline poll loads the symbol as an
 * ordinary external `i32`, like the old flags. Defined on BOTH builds because the
 * monitor (shared) publishes the PREEMPT lane; under Boehm the GC lanes are
 * unused and compiled code never reads the word (Boehm preempts via a direct
 * signal switch). */
volatile uint32_t zap_safepoint_flags = 0;
#define ZAP_SP_GC_WANTED 0x1u  /* a collection is due (set by alloc)        */
#define ZAP_SP_COLLECTING 0x2u /* a collection is in progress               */
#define ZAP_SP_PREEMPT 0x4u    /* some M has a pending fairness preemption   */

/* Lane accessors over `zap_safepoint_flags`. `zap_sp_test(mask)` is true when ANY
 * bit of `mask` is set, so a combined check like the old `zap_gc_wanted ||
 * zap_collecting` is one masked load: `zap_sp_test(ZAP_SP_GC_WANTED |
 * ZAP_SP_COLLECTING)`. Set/clear are atomic RMW so a concurrent writer of another
 * lane is never lost. */
static inline int zap_sp_test(uint32_t mask) {
    return (__atomic_load_n(&zap_safepoint_flags, __ATOMIC_RELAXED) & mask) != 0;
}
static inline void zap_sp_set(uint32_t lane) {
    __atomic_fetch_or(&zap_safepoint_flags, lane, __ATOMIC_RELAXED);
}
static inline void zap_sp_clear(uint32_t lane) {
    __atomic_fetch_and(&zap_safepoint_flags, ~lane, __ATOMIC_RELAXED);
}

#if defined(ZAP_PLATFORM_POSIX)
/* SIGURG handler: the preemption point. Installed process-wide; only acts on a
 * worker thread that is currently running a (non-pinned) coroutine. */
/* I#891 / I#223 defense in depth: is the interrupted instruction inside THIS
 * program's own text (the Zap-generated code and the runtime, which are linked into
 * the executable)? If not, the signal landed inside libgc, libc, the dynamic loader
 * or the vdso — code that may hold the allocator lock or other thread-affine state
 * (the collector's thread-local free lists, mutexes whose owner is the OS thread),
 * and switching the coroutine away THERE lets the next coroutine on this thread
 * corrupt or deadlock on it. A 32-core aarch64 re-run measured the effect: no
 * preemption unless the PC is in the executable's text took a ~1.5% failure rate to
 * 0/4500. The no-preempt windows (zap_np_*) are the precise guard; this is the
 * catch-all for libc/libgc code reached on paths nobody wrapped. A declined
 * preemption keeps `m->preempt` latched and the monitor re-signals, so fairness only
 * waits for the next tick. Linux x86_64/aarch64 only; elsewhere it always allows. */
#ifndef ZAP_GC_IMMIX /* only the Boehm async-preemption branch consults it */
#if defined(__linux__) && (defined(__x86_64__) || defined(__aarch64__))
/* WEAK: when the runtime is built as a shared object (the REPL `dlopen`s it, and
 * `-fPIC` builds exist) the linker does not provide `__executable_start`, and a strong
 * reference would make the load fail ("undefined symbol"). Missing bounds mean we
 * cannot tell "program text" from "library text", so preemption is simply allowed
 * (the no-preempt windows remain the precise guard). */
extern char __executable_start __attribute__((weak));
extern char etext __attribute__((weak));
static int zap_pc_in_program_text(void *uc) {
    if (&__executable_start == NULL || &etext == NULL) {
        return 1;
    }
    ucontext_t *u = (ucontext_t *)uc;
#if defined(__x86_64__)
    uintptr_t pc = (uintptr_t)u->uc_mcontext.gregs[16]; /* REG_RIP (needs _GNU_SOURCE by name) */
#else
    uintptr_t pc = (uintptr_t)u->uc_mcontext.pc;
#endif
    return pc >= (uintptr_t)&__executable_start && pc < (uintptr_t)&etext;
}
#else
static int zap_pc_in_program_text(void *uc) {
    (void)uc;
    return 1;
}
#endif
#endif /* !ZAP_GC_IMMIX */

static void zap_preempt_handler(int sig, siginfo_t *info, void *uc) {
    (void)sig;
    (void)info;
    M *m = cur_m;
    if (!m || !m->current) {
        return; /* main/scheduler/idle thread: nothing to preempt */
    }
    if (m->current->locked_m) {
        return; /* a pinned coroutine owns its M — never fairness-preempt it */
    }
    /* observed at the next safepoint (precise GC); relaxed atomic — advisory
     * latch, no payload, and async-signal-safe (#213, D-0121) */
    __atomic_store_n(&m->preempt, 1, __ATOMIC_RELAXED);
#ifndef ZAP_GC_IMMIX
    /* Conservative GC: switch at the interrupted point directly — but ONLY when
     * we actually interrupted the coroutine running on its green stack. The
     * signal can also land while this worker is on its NATIVE stack inside the
     * scheduler (switch_to_g has set m->current but not yet swapped the context
     * in, or is just back from a swap): switching there would clobber the
     * coroutine's saved context. Detect by testing the interrupted stack pointer
     * against the green-stack range. Also require no runtime lock / allocation to
     * be held (no_preempt); otherwise defer and let the monitor re-signal.
     *
     * The STW guard (`g_boehm_in_stw`) is read LAST — after the cheap SP/no_preempt
     * checks — via a seq-cst acquire load, so the check->act window (the span
     * between seeing "no STW" and committing to the swap) is the few instructions
     * before `copreempt`, and the read is correctly ordered against the release
     * store in `zap_boehm_gc_event` (so a weak-memory arch cannot see a stale 0
     * after STW began — #693/#213, D-0120). If STW nonetheless begins inside that
     * residual window, the swap is still GC-root-safe: the registers live at the
     * suspend are captured by bdwgc's STW register snapshot, and the coroutine's
     * stack is its green stack — a GC_MALLOC object reachable through its `G` in
     * g_all and so conservatively scanned as heap no matter which stack bottom the
     * collector read (the same invariant that makes the lock-free switch-path
     * bottom update safe, D-0117). The flag is thus a fairness belt-and-suspenders,
     * not a correctness lock. POSIX deliberately does NOT mirror Windows'
     * `g_preempt_stw_mu` here: `pthread_mutex_lock` is not async-signal-safe, and a
     * lock held across a stack swap that resumes on a different coroutine could
     * never be released on this flow. */
    {
        G *g = m->current;
        char probe;
        char *sp = &probe;
        char *lo = (char *)g->stack;
        char *hi = lo + g->stack_size;
        if (sp >= lo && sp < hi && g->no_preempt == 0 && zap_pc_in_program_text(uc) &&
            !__atomic_load_n(&g_boehm_in_stw, __ATOMIC_SEQ_CST)) {
            __atomic_store_n(&m->preempt, 0, __ATOMIC_RELAXED);
            copreempt();
        }
    }
#endif
}
#endif /* ZAP_PLATFORM_POSIX */

/* The monitor loop: a plain OS thread (it never touches the GC heap, holds no
 * lock a collection needs, and blocks SIGURG on itself). Started with
 * GC_pthread_create so bdwgc accounts for it. */
static void *zap_monitor_main(void *arg) {
    (void)arg;
#if defined(ZAP_PLATFORM_POSIX)
    sigset_t block;
    sigemptyset(&block);
    sigaddset(&block, SIGURG);
    pthread_sigmask(SIG_BLOCK, &block, NULL);
#endif
    uint64_t *last = (uint64_t *)calloc((size_t)g_nm, sizeof(uint64_t));
    if (!last) {
        return NULL;
    }
#if !defined(ZAP_PLATFORM_WINDOWS)
    /* `ZAP_PREEMPT_SLICE_US` shortens the preemption slice (default ~10 ms) so a
     * test can make the async-preemption signal land thousands of times more
     * often and exercise the park/exit/collect windows reliably (I#888, I#889).
     * A tuning/testing knob; unset or out of range (20 us .. 1 s) keeps the default. */
    long slice_ns = ZAP_PREEMPT_SLICE_NS;
    {
        const char *e = getenv("ZAP_PREEMPT_SLICE_US");
        if (e && *e) {
            long us = strtol(e, NULL, 10);
            if (us >= 20 && us < 1000000L) {
                slice_ns = us * 1000L;
            }
        }
    }
#endif
    for (;;) {
#if defined(ZAP_PLATFORM_WINDOWS)
        Sleep((DWORD)(ZAP_PREEMPT_SLICE_NS / 1000000L));
#else
        struct timespec ts;
        ts.tv_sec = 0;
        ts.tv_nsec = slice_ns;
        nanosleep(&ts, NULL);
#endif
        if (__atomic_load_n(&g_shutdown, __ATOMIC_RELAXED)) {
            break;
        }
        /* OR of every M's preempt latch this tick, published to the PREEMPT lane of
         * `zap_safepoint_flags` after the scan so compiled code's inline poll sees a
         * pending preemption and clears it once all are consumed (#568, D-0144;
         * coalesced lane, #860, D-0155). */
        int preempt_any = 0;
        for (int i = 0; i < g_nm; i++) {
            M *m = &g_ms[i];
            /* Read without a lock: `current` is only tested for non-NULL (the
             * handler re-checks on the worker thread), and `schedtick` is read
             * via a RELAXED atomic — a well-defined cross-thread load with no
             * ordering dependence, correct on weak-memory arches too (#213,
             * D-0121), not the prior plain `volatile` that relied on the access
             * happening to be atomic on the supported arch. */
            uint64_t tick = __atomic_load_n(&m->schedtick, __ATOMIC_RELAXED);
            if (m->current && tick == last[i]) {
                __atomic_store_n(&m->preempt, 1, __ATOMIC_RELAXED);
#if defined(ZAP_PLATFORM_WINDOWS)
                /* No SIGURG: hijack the worker directly. Snapshot the green-stack
                 * range of the coroutine we believe is running; zap_win_preempt_try
                 * re-validates RSP ∈ [lo,hi) and no_preempt AFTER suspending, so a
                 * stale read (the coroutine parked between here and the suspend) is
                 * declined, not mis-hijacked. Hold the STW mutex so this never races
                 * bdwgc's own SuspendThread (Q5). Skip pinned coroutines. */
                G *g = m->current;
                if (g && !g->locked_m && m->win_thread) {
                    uintptr_t lo = (uintptr_t)g->stack;
                    uintptr_t hi = lo + (uintptr_t)g->stack_size;
                    pthread_mutex_lock(&g_preempt_stw_mu);
                    zap_win_preempt_try(m->win_thread, lo, hi, &g->no_preempt,
                                        &m->in_hijack);
                    pthread_mutex_unlock(&g_preempt_stw_mu);
                }
#elif defined(ZAP_GC_IMMIX)
                pthread_kill(m->thread, SIGURG);
#else
                /* gc-boehm (#710, D-0128): never deliver the async-preempt
                 * signal while a bdwgc stop-the-world is in progress. The monitor
                 * runs on its own thread; a SIGURG landing on a worker that the
                 * collector is mid-suspending races bdwgc's STW suspend/ack
                 * handshake (observed hang: `main` spinning in GC_generic_malloc_many
                 * with every worker parked in bdwgc's suspend handler), and the
                 * handler's copreempt stack-swap can move a mutator out from under
                 * the collector during mark (the MPSC queue-node use-after-free:
                 * `unwrap on an Optional.None value`). The in-handler guard added
                 * for #693/#213 (D-0120) re-reads `g_boehm_in_stw` before copreempt,
                 * but the monitor itself did not, so it kept firing INTO a running
                 * collection — this gate closes that at the source. immix has no
                 * signal-based STW, so its branch above is unchanged. Causation was
                 * proven on a 32-vCPU x86 box: neutering this signal ran the coro
                 * stress 800+ iters clean vs. hang/crash at ~iter 100 with it on
                 * (MAXPROCS=64, GC_FREE_SPACE_DIVISOR=3). */
                if (!__atomic_load_n(&g_boehm_in_stw, __ATOMIC_SEQ_CST)) {
                    pthread_kill(m->thread, SIGURG);
                }
#endif
            }
            last[i] = tick;
            preempt_any |= __atomic_load_n(&m->preempt, __ATOMIC_RELAXED);
        }
        /* Publish the PREEMPT lane (#568, #860). The monitor is the sole writer of
         * THIS lane, but the GC lanes of the shared word are written concurrently
         * by the allocator and collector, so the lane is updated via an atomic RMW
         * (not a whole-word store that would clobber a concurrent GC-lane write). A
         * just-flagged M (set above in this same scan) is included, and the lane
         * drops to 0 only on a tick where every latch reads 0 — i.e. nothing is
         * genuinely pending. */
        if (preempt_any) {
            zap_sp_set(ZAP_SP_PREEMPT);
        } else {
            zap_sp_clear(ZAP_SP_PREEMPT);
        }
    }
    free(last);
    return NULL;
}

/* Install the preemption primitive once. POSIX: the SIGURG handler (no SA_ONSTACK
 * — the handler must run on the coroutine's own green stack so a copreempt switch
 * saves/restores it with the coroutine's context). Windows: point the A3 hijack
 * trampoline at copreempt (the monitor does the suspend/redirect). Idempotent. */
static void zap_preempt_install(void) {
#if defined(ZAP_PLATFORM_WINDOWS)
    zap_win_preempt_target = copreempt;
#else
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = zap_preempt_handler;
    sa.sa_flags = SA_SIGINFO | SA_RESTART;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGURG, &sa, NULL);
#endif
}

/* =====================  LockOSThread (ticket #172)  ========================
 * `runtime.lock_os_thread()` pins the calling coroutine to a dedicated OS
 * thread (its own M) until the matching `runtime.unlock_os_thread()`: no other
 * coroutine runs on that thread, and the coroutine always resumes on it (across
 * channel parks too). It is the Go `runtime.LockOSThread` analogue — for C
 * libraries that keep thread-local state and for long jobs that want off the
 * shared pool — and the substrate the later threading-compat library reuses.
 *
 * Mechanism: a dedicated M runs a single pinned coroutine via a private slot
 * (`ded_g`/`ded_runnable`), bypassing the work-stealing pool; `sched_enqueue`
 * routes a pinned coroutine back to it. The dedicated M is excluded from
 * fairness preemption (not in `g_ms[]`, and `locked_m != NULL` short-circuits
 * `copreempt`), still polls GC safepoints, and tears down when the coroutine
 * unlocks or finishes. `lock_depth` makes nested lock/unlock balance. */
static pthread_mutex_t g_ded_mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t g_ded_cv = PTHREAD_COND_INITIALIZER;
static int64_t g_ded_live = 0; /* dedicated Ms not yet torn down (join barrier) */

static void *dedicated_worker_main(void *arg) {
    M *m = (M *)arg;
    cur_m = m;
    struct GC_stack_base sb;
    (void)GC_get_my_stackbottom(&sb);
    m->native_sb = sb;
    zap_mutator_register();
#if defined(ZAP_PLATFORM_WINDOWS)
    /* Give this thread the fault-handler stack headroom; a pinned coroutine owns
     * its M and is never hijacked by the monitor (it is not in g_ms). */
    zap_win_fault_thread_init();
#else
    /* A pinned coroutine is not fairness-preempted, so keep SIGURG blocked on
     * the dedicated thread (it still participates in GC via safepoints). */
    sigset_t block;
    sigemptyset(&block);
    sigaddset(&block, SIGURG);
    pthread_sigmask(SIG_BLOCK, &block, NULL);
#endif
    for (;;) {
        zap_enter_blocking(); /* idle: GC-safe, no un-flushed roots */
        zap_np_lock(&m->ded_mu);
        while (!m->ded_runnable && !m->ded_stop) {
            pthread_cond_wait(&m->ded_cv, &m->ded_mu);
        }
        int stop = m->ded_stop && !m->ded_runnable;
        m->ded_runnable = 0;
        zap_np_unlock(&m->ded_mu);
        if (stop) {
            zap_leave_blocking();
            break;
        }
        zap_leave_blocking();
        G *g = m->ded_g;
        switch_to_g(m, g);
        /* switch_to_g handled the outcome: a park/yield re-routed the coroutine
         * back to us (locked) or to the shared pool (unlocked); death reclaimed
         * it. We own this thread only while the coroutine is still pinned here. */
        if (g->state == G_DEAD || g->locked_m != m) {
            break;
        }
    }
    /* Mark GC-safe before leaving so a collection in flight never waits on this
     * now-exiting thread, then drop our mutator node (precise GC). */
    zap_enter_blocking();
    zap_mutator_unregister();
    if (m->ded_joinable) {
        zap_np_lock(&g_ded_mu);
        if (--g_ded_live == 0) {
            pthread_cond_broadcast(&g_ded_cv);
        }
        zap_np_unlock(&g_ded_mu);
    }
    free(m);
    return NULL;
}

/* Pin the calling coroutine to a fresh dedicated M. `joinable` selects the
 * program-exit barrier: 1 (LockOSThread, #172) counts the dedicated M in
 * `g_ded_live` so `zap_rt_join_all` waits for it; 0 (threading-compat, #175)
 * leaves it out of `g_ded_live` — non-daemon threads are instead awaited via
 * `g_live` (registered synchronously in `Thread.start`), and daemon threads are
 * awaited by nothing, so neither blocks exit through this M. */
static void dedicated_pin(int joinable) {
    M *m = cur_m;
    if (!m || !m->current) {
        return; /* not on a coroutine (e.g. main): no-op, like Go on g0 */
    }
    G *g = m->current;
    if (g->locked_m) {
        g->lock_depth++;
        return; /* already pinned: nest */
    }
    M *d = (M *)calloc(1, sizeof(M));
    if (!d) {
        return;
    }
    d->dedicated = 1;
    d->ded_joinable = joinable;
    d->p = NULL; /* no work-stealing P: only ever runs this one coroutine */
    pthread_mutex_init(&d->ded_mu, NULL);
    pthread_cond_init(&d->ded_cv, NULL);
    d->ded_g = g;
    if (joinable) {
        zap_np_lock(&g_ded_mu);
        g_ded_live++;
        zap_np_unlock(&g_ded_mu);
    }
    if (zap_gc_pthread_create(&d->thread, NULL, dedicated_worker_main, d) != 0) {
        if (joinable) {
            zap_np_lock(&g_ded_mu);
            g_ded_live--;
            zap_np_unlock(&g_ded_mu);
        }
        free(d);
        return;
    }
    zap_gc_pthread_detach(d->thread); /* self-cleaning; joined via the chosen barrier */
    g->lock_depth = 1;
    g->locked_m = d;
    /* Yield: switch_to_g on the current worker will route us (locked_m set) to
     * the dedicated M, where we resume. From here on this coroutine runs only
     * on `d`. */
    cosched();
}

/* `runtime.lock_os_thread()` — pin the calling coroutine to a dedicated M. */
void zap_runtime_lock_os_thread(void) { dedicated_pin(1); }

/* `threading._pin()` (#175) — pin a threading-compat Thread's coroutine to a
 * dedicated OS thread that is NOT counted in the LockOSThread join barrier.
 * Unpinned by the shared `zap_runtime_unlock_os_thread`. */
void zap_threading_pin(void) { dedicated_pin(0); }

/* `runtime.unlock_os_thread()` — release the pin (balances nested locks). */
void zap_runtime_unlock_os_thread(void) {
    M *m = cur_m;
    if (!m || !m->current) {
        return;
    }
    G *g = m->current;
    if (!g->locked_m || g->lock_depth == 0) {
        return; /* not pinned: no-op */
    }
    if (--g->lock_depth > 0) {
        return; /* still nested */
    }
    M *d = g->locked_m;
    g->locked_m = NULL;
    /* Yield: switch_to_g on the dedicated M sees locked_m == NULL, routes us to
     * the shared pool, then (coroutine no longer pinned here) tears `d` down. */
    cosched();
    (void)d;
}

/* The effective MAXPROCS (worker-M count). Exposed as `runtime.maxprocs()`. */
int64_t zap_runtime_maxprocs(void) {
    zap_sched_init(0); /* materialize the default if nothing spawned yet */
    return (int64_t)g_nm;
}

static void zap_sched_init_once(void) {
#if defined(ZAP_PLATFORM_POSIX) && !defined(ZAP_GC_IMMIX)
    /* I#885: before any worker can run a coroutine. See `zap_sp_corrector`. */
    GC_set_sp_corrector(zap_sp_corrector);
#endif
    g_nm = zap_maxprocs();
    g_ps = (P *)ZAP_INFRA_ALLOC((size_t)g_nm * sizeof(P));
    g_ms = (M *)ZAP_INFRA_ALLOC((size_t)g_nm * sizeof(M));
    for (int i = 0; i < g_nm; i++) {
        pthread_mutex_init(&g_ps[i].mu, NULL);
        g_ps[i].head = g_ps[i].tail = NULL;
        memset(&g_ms[i], 0, sizeof(M));
        g_ms[i].p = &g_ps[i];
    }
    /* Install the async-preemption handler BEFORE the workers start, so they
     * inherit it; SIGURG stays unblocked on workers (default), the monitor and
     * non-worker threads block it (ticket #172). */
    zap_preempt_install();
#ifndef ZAP_GC_IMMIX
    /* Bracket Boehm stop-the-world so the handler never switches a thread the
     * collector is suspending (see zap_boehm_gc_event). */
    GC_set_on_collection_event(zap_boehm_gc_event);
#endif
    for (int i = 0; i < g_nm; i++) {
        int rc = zap_gc_pthread_create(&g_ms[i].thread, NULL, worker_main, &g_ms[i]);
        if (rc != 0) {
            fprintf(stderr, "zap: failed to start scheduler thread (%d)\n", rc);
            abort();
        }
    }
    /* The preemption monitor: samples schedticks and signals monopolizing Ms. */
    if (zap_gc_pthread_create(&g_monitor_thread, NULL, zap_monitor_main, NULL) == 0) {
        g_monitor_started = 1;
    }
    /* ZAP_PROFILE=cpu: start the CPU profiler now that the worker Ms it samples
     * exist (ticket #78). A no-op unless the env requested it. */
    zap_profile_env_maybe_start();
}

/* `zap_sched_init(n_threads)` — the design's named entry point. A value <= 0
 * means "use the default" (ZAP_MAXPROCS / hardware threads). The scheduler is
 * started at most once; an explicit n only takes effect if it is the first call
 * (before any spawn), honoring the env override otherwise. */
void zap_sched_init(int64_t n_threads) {
    if (n_threads > 0 && !g_ms) {
        /* Seed the env-less default so zap_maxprocs picks this up. */
        static char buf[32];
        if (!getenv("MAXPROCS")) {
            snprintf(buf, sizeof(buf), "%lld", (long long)n_threads);
            zap_setenv_compat("MAXPROCS", buf, 0);
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
    zap_np_lock(&g_live_mu);
    g_live++;
    zap_np_unlock(&g_live_mu);
    G *g = galloc(fn_, arg);
    g->in_live = 1; /* joined by the implicit root scope at program exit */
    zap_race_on_spawn(g); /* --race: parent->child happens-before edge (#678) */
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
    zap_race_on_spawn(g); /* --race: parent->child happens-before edge (#678) */
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
    zap_np_lock(&g_live_mu);
    while (g_live > 0) {
        pthread_cond_wait(&g_live_cv, &g_live_mu);
    }
    zap_np_unlock(&g_live_mu);
    zap_leave_blocking();
    /* Every coroutine has finished: stop the CPU profiler and flush its folded
     * output (ticket #78). Done here, before the workers are joined, so a
     * whole-run `ZAP_PROFILE=cpu` profile is written on normal program exit. No
     * coroutine is on-CPU any more, so the monitor has nothing left to sample. */
    zap_profile_stop();
    /* Every pinned coroutine has finished (g_live hit 0 above), so every
     * dedicated LockOSThread M is tearing down; wait for them before stopping
     * the shared pool (ticket #172). */
    zap_np_lock(&g_ded_mu);
    while (g_ded_live > 0) {
        pthread_cond_wait(&g_ded_cv, &g_ded_mu);
    }
    zap_np_unlock(&g_ded_mu);
    zap_np_lock(&g_idle_mu);
    __atomic_store_n(&g_shutdown, 1, __ATOMIC_RELAXED);
    pthread_cond_broadcast(&g_idle_cv);
    zap_np_unlock(&g_idle_mu);
    /* Stop the preemption monitor (it polls g_shutdown every slice). */
    if (g_monitor_started) {
        zap_gc_pthread_join(g_monitor_thread, NULL);
        g_monitor_started = 0;
    }
    for (int i = 0; i < g_nm; i++) {
        zap_gc_pthread_join(g_ms[i].thread, NULL);
    }
    /* All worker Ms are joined, so no SIGPROF handler can still be running: free
     * the per-M profiler ring buffers (ticket #78). */
    zap_profile_teardown();
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
 * a scope failure. Deadline, signal, failure, and early-exit cancellation
 * share one channel-aware wake path. Nested scope joins park their G. */

typedef struct Scope {
    pthread_mutex_t mu;
    pthread_cond_t cv;      /* broadcast when `live` reaches 0 (join barrier)   */
    int64_t live;          /* owned coroutines not yet finished                */
    int cancelled;         /* cancellation requested (armed cooperatively)     */
    int timed_out;
    struct Scope *parent;  /* immutable ancestry; parent awaits this scope's G */
    Waiter *joiners;
    int on_error;          /* 0 = Cancel (default), 1 = Wait (D-0044)          */
    void **excs;           /* collected typed exceptions from owned coroutines */
    int64_t nexc;
    int64_t exc_cap;
    /* Deadline reporting (I#475). Captured by the deadline at the moment it
     * fires — i.e. the complete set of coroutines still outstanding — and read
     * back when `ScopeTimeout` is built. `timeout_msg` is a malloc'd C buffer
     * (not GC-managed), freed when the exception is built. */
    int64_t deadline_ms;
    char *timeout_msg;
    int64_t timeout_msg_len;
} Scope;

/* The shared `Cancelled` sentinel (a BaseException-ish marker, design/0005): a
 * single cached ZapStr whose pointer identity marks a cooperative cancel, so a
 * cancelled sibling is told apart from a genuine failure by identity, not text.
 * Held in a GC-scanned static, so it stays live. */
static void *g_cancelled = NULL;
static pthread_mutex_t g_cancelled_mu = PTHREAD_MUTEX_INITIALIZER;
static void *cancelled_exc(void) {
    zap_np_lock(&g_cancelled_mu);
    if (!g_cancelled) {
        g_cancelled = zap_exception_new(ZAP_EXC_CANCELLED, zap_str_new("Cancelled", 9));
    }
    zap_np_unlock(&g_cancelled_mu);
    return g_cancelled;
}

/* Yield the current coroutine back to the scheduler (a cooperative scheduling
 * point). A no-op on a native thread (e.g. `main`), which does not run on an M.
 * The scheduler requeues a G left in G_RUNNABLE (see `switch_to_g`). */
static void cosched(void) {
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
    /* A yield is a safe point: any no-preempt window the caller opened ends here
     * (it holds no lock at a cooperative yield) — but the depth is cleared by
     * `switch_to_g` AFTER the swap, not here, so the state-store -> swap span
     * stays non-preemptible (I#889, see `g_enter_switch_out`). */
    g_enter_switch_out(g);
    g->state = G_RUNNABLE;
    zap_ctx_swap(&g->ctx, &m->sched_ctx);
}

/* Preemptive yield (ticket #172): like `cosched`, but asks the scheduler to
 * requeue this coroutine on the GLOBAL queue (behind its peers) so a long
 * CPU-bound coroutine cannot re-grab its M immediately. Invoked from a safepoint
 * (precise GC) or the async-preemption signal handler (Boehm). */
static void copreempt(void) {
    M *m = cur_m;
    if (!m || !m->current) {
        return;
    }
    G *g = m->current;
    /* A LockOSThread-pinned coroutine owns its M outright — never preempt it for
     * fairness (it is not sharing the thread with anyone). */
    if (g->locked_m) {
        return;
    }
    /* No stack-redzone probe here (unlike cosched): a preemptive yield grows no
     * user stack — it only switches away — and this can run in signal context
     * (Boehm), where zap_panic would be unsafe. A genuinely near-full stack is
     * still caught at the next cooperative cosched/copark in normal context. */
    g_enter_switch_out(g); /* I#889; cleared by switch_to_g after the swap */
    g->state = G_RUNNABLE;
    g->preempt_requeue = 1;
#if defined(ZAP_PLATFORM_WINDOWS)
    /* The coroutine has now yielded (we are about to swap away): clear the per-M
     * in-flight guard so the monitor may hijack this M again on a later slice —
     * the exact analogue of the Unix handler returning and unblocking SIGURG
     * (ticket #197 Q5). The swap below happens immediately, so the window before
     * it in which the ~10 ms monitor could re-fire is effectively nil. */
    InterlockedExchange(&m->in_hijack, 0);
#endif
    zap_ctx_swap(&g->ctx, &m->sched_ctx);
}

/* Is the current coroutine's owning scope cancelled? (0 on a native thread or an
 * unowned coroutine.) */
static int g_is_cancelled(void) {
    M *m = cur_m;
    G *g = m ? m->current : NULL;
    if (!g || !g->scope) {
        return 0;
    }
    for (Scope *sc = g->scope; sc; sc = sc->parent) {
        zap_np_lock(&sc->mu);
        int cancelled = sc->cancelled;
        zap_np_unlock(&sc->mu);
        if (cancelled) return 1;
    }
    return 0;
}

/* A cancellation point: raise `Cancelled` if the current coroutine was armed. */
static void zap_check_cancel(void) {
    if (g_is_cancelled()) {
        zap_raise(cancelled_exc());
    }
}

/* `checkpoint()` (D-0044): an explicit cancellation point for long CPU-bound
 * loops. Observes a pending cancel, then yields so siblings make progress. */
void zap_checkpoint(void) {
    zap_check_cancel();
    cosched();
}

/* An owned coroutine finished: drop it from its scope's live count and wake the
 * join barrier when the last one leaves. */
static void coready(Waiter *w);
static void copark(pthread_mutex_t *mu, Waiter *w);
static void scope_gdone(Scope *sc) {
    zap_np_lock(&sc->mu);
    if (--sc->live == 0) {
        pthread_cond_broadcast(&sc->cv);
        while (sc->joiners) {
            Waiter *w = sc->joiners; sc->joiners = w->next;
            coready(w);
        }
    }
    zap_np_unlock(&sc->mu);
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
    sc->timed_out = 0;
    sc->joiners = NULL;
    sc->parent = cur_m && cur_m->current ? cur_m->current->scope : NULL;
    sc->on_error = on_error == 1 ? 1 : 0;
    sc->excs = NULL;
    sc->nexc = 0;
    sc->exc_cap = 0;
    sc->deadline_ms = 0;
    sc->timeout_msg = NULL;
    sc->timeout_msg_len = 0;
    return sc;
}

/* Lexically nested scopes on main also belong to their enclosing scope. */
void zap_scope_set_parent(void *scope_, void *parent_) {
    ((Scope *)scope_)->parent = (Scope *)parent_;
}
static void scope_request_cancel(Scope *sc); /* shared channel-aware cancel */
static void *scope_timeout_exc(Scope *sc);    /* build ScopeTimeout w/ pending set */
/* Release the captured pending-set buffer when a deadline fired but `ScopeTimeout`
 * is NOT what surfaces (a child failure outranks it), so the malloc'd report does
 * not leak. */
static void scope_discard_timeout_msg(Scope *sc) {
    zap_np_lock(&sc->mu);
    char *buf = sc->timeout_msg;
    sc->timeout_msg = NULL;
    sc->timeout_msg_len = 0;
    zap_np_unlock(&sc->mu);
    free(buf);
}

/* `spawn <call>` inside a `scope:` (ticket #50): a coroutine owned by `scope`,
 * awaited at the scope's dedent rather than at program exit. `name` is a static
 * C-string identifying the spawn target, carried for `ScopeTimeout` reports and
 * the coroutine dump (I#475 / D-0058). */
void zap_spawn_scoped(void *fn_, void *arg, void *scope_, const char *name) {
    zap_sched_init(0);
    Scope *sc = (Scope *)scope_;
    zap_np_lock(&sc->mu);
    sc->live++;
    zap_np_unlock(&sc->mu);
    G *g = galloc(fn_, arg);
    g->scope = sc; /* in_live stays 0: the scope, not the root join, owns it */
    g->name = name;
    zap_race_on_spawn(g); /* --race: parent->child happens-before edge (#678) */
    sched_enqueue(g);
}

/* Block until every coroutine owned by `sc` has finished. Called from `main`
 * (a native thread) for a top-level scope; a scope reached from inside a
 * coroutine parks its G so a single worker can run the owned children. */
static void scope_join(Scope *sc) {
    if (cur_m && cur_m->current) {
        cur_m->current->park_reason = "nested scope"; /* I#475 diagnostics */
        zap_np_lock(&sc->mu);
        while (sc->live > 0) {
            Waiter w = {0};
            w.next = sc->joiners; sc->joiners = &w;
            copark(&sc->mu, &w);
            zap_np_lock(&sc->mu);
        }
        zap_np_unlock(&sc->mu);
    } else {
        zap_enter_blocking();
        zap_np_lock(&sc->mu);
        while (sc->live > 0) pthread_cond_wait(&sc->cv, &sc->mu);
        zap_np_unlock(&sc->mu);
        zap_leave_blocking();
    }
}

/* Order the collected failures deterministically (by message bytes) so an
 * `ExceptionGroup` built from concurrent failures has a stable rendering. */
static int exc_cmp(const void *a, const void *b) {
    const ZapStr *x = (const ZapStr *)zap_exception_message(*(void *const *)a);
    const ZapStr *y = (const ZapStr *)zap_exception_message(*(void *const *)b);
    int64_t n = x->len < y->len ? x->len : y->len;
    int c = n > 0 ? memcmp(x->data, y->data, (size_t)n) : 0;
    if (c != 0) {
        return c;
    }
    return x->len < y->len ? -1 : (x->len > y->len ? 1 : 0);
}

/* Build the aggregate `ExceptionGroup` message for `n (>= 2)` concurrent
 * failures (D-0045); the caller wraps it in the typed exception. */
static void *make_exception_group(void **excs, int64_t n) {
    void **sorted = (void **)zap_ptrbuf((int64_t)n * (int64_t)sizeof(void *));
    memcpy(sorted, excs, (size_t)n * sizeof(void *));
    qsort(sorted, (size_t)n, sizeof(void *), exc_cmp);
    char header[64];
    int hlen = snprintf(header, sizeof(header),
                        "ExceptionGroup (%lld sub-exceptions):", (long long)n);
    int64_t total = hlen;
    for (int64_t i = 0; i < n; i++) {
        ZapStr *msg = (ZapStr *)zap_exception_message(sorted[i]);
        total += 5 + (msg ? msg->len : 0); /* "\n  - " + message */
    }
    char *buf = (char *)zap_buf((size_t)total);
    int64_t off = 0;
    memcpy(buf + off, header, (size_t)hlen);
    off += hlen;
    for (int64_t i = 0; i < n; i++) {
        memcpy(buf + off, "\n  - ", 5);
        off += 5;
        ZapStr *msg = (ZapStr *)zap_exception_message(sorted[i]);
        if (msg && msg->len > 0) {
            memcpy(buf + off, msg->data, (size_t)msg->len);
            off += msg->len;
        }
    }
    return zap_str_new(buf, off);
}

/* ---- ExceptionGroup split / inspect for `except*` (#605, D-0129) ----------- *
 * An ExceptionGroup carries its real sub-exception objects in `children` (a
 * ZapList of exception pointers). `except*` peels matching sub-exceptions out of
 * a mutable working set into typed sub-groups, re-raising the remainder;
 * `eg.exceptions` iterates the sub-exception messages. Children are kept in a
 * stable (sorted-by-message) order so the rendered message, `.exceptions`, and
 * the goldens are deterministic regardless of coroutine scheduling. */

/* Build an ExceptionGroup carrying `list_` (a ZapList of exception objects). The
 * children are sorted in place to match `make_exception_group`'s rendering. */
void *zap_eg_new(void *list_) {
    ZapList *l = (ZapList *)list_;
    if (l->len > 1) {
        qsort(l->data, (size_t)l->len, sizeof(int64_t), exc_cmp);
    }
    void *msg = make_exception_group((void **)l->data, l->len);
    ZapException *e = (ZapException *)zap_exception_new(ZAP_EXC_EXCEPTION_GROUP, msg);
    e->children = l;
    l->elem_ptr = 1; /* precise GC: the children words are exception pointers */
    return e;
}

/* Build an ExceptionGroup from a C array of `n` exception objects (the scope
 * join path, D-0045): copy them into a ZapList, then wrap as a carrying group. */
static void *build_exception_group(void **excs, int64_t n) {
    ZapList *l = (ZapList *)zap_list_new();
    for (int64_t i = 0; i < n; i++) {
        zap_list_push_ptr(l, excs[i]);
    }
    return zap_eg_new(l);
}

/* The working "remaining" set for `except*`: a fresh ZapList of the still-
 * unhandled sub-exceptions. A group contributes a shallow copy of its children
 * (so the original group is never mutated); a bare exception contributes itself
 * as a one-element list, so a single failure matches `except* E` (D-0129). */
void *zap_eg_remaining(void *exc) {
    ZapList *out = (ZapList *)zap_list_new();
    ZapException *e = (ZapException *)exc;
    if (e && e->magic == ZAP_EXCEPTION_MAGIC && e->type_id == ZAP_EXC_EXCEPTION_GROUP
        && e->children) {
        ZapList *ch = (ZapList *)e->children;
        for (int64_t i = 0; i < ch->len; i++) {
            zap_list_push_ptr(out, (void *)(intptr_t)ch->data[i]);
        }
    } else {
        zap_list_push_ptr(out, exc);
    }
    return out;
}

/* Peel every remaining sub-exception matching `type_id` into a fresh sub-group,
 * removing them from `remaining_` (compacted in place). Returns the sub-group
 * (always an ExceptionGroup, even for a single match), or NULL when nothing
 * matched — then the clause's handler is skipped (#605, D-0129). */
void *zap_eg_split(void *remaining_, int64_t type_id) {
    ZapList *rem = (ZapList *)remaining_;
    ZapList *matched = (ZapList *)zap_list_new();
    int64_t w = 0;
    for (int64_t r = 0; r < rem->len; r++) {
        void *child = (void *)(intptr_t)rem->data[r];
        if (zap_exception_is_a(child, type_id)) {
            zap_list_push_ptr(matched, child);
        } else {
            rem->data[w++] = rem->data[r];
        }
    }
    rem->len = w;
    if (matched->len == 0) {
        return NULL;
    }
    return zap_eg_new(matched);
}

/* After all `except*` clauses: decide what the unmatched remainder re-raises.
 * Nothing left -> NULL (no re-raise); one survivor -> that exception bare (so a
 * plain enclosing `except E` catches it, keeping Zap's bare-for-single rule);
 * two or more -> a fresh ExceptionGroup of the remainder (#605, D-0129). */
void *zap_eg_finalize_remainder(void *remaining_) {
    ZapList *rem = (ZapList *)remaining_;
    if (rem->len == 0) {
        return NULL;
    }
    if (rem->len == 1) {
        return (void *)(intptr_t)rem->data[0];
    }
    return zap_eg_new(rem);
}

/* `eg.exceptions` (#605): the sub-exception message strings of an exception, as
 * a `list[str]` for manual inspection / iteration. A group yields one entry per
 * child in its stable order; a bare exception yields a one-element list of its
 * own message, so inspection is uniform whether or not a group was caught. */
void *zap_exception_exceptions(void *exc) {
    ZapList *out = (ZapList *)zap_list_new();
    ZapException *e = (ZapException *)exc;
    if (e && e->magic == ZAP_EXCEPTION_MAGIC && e->type_id == ZAP_EXC_EXCEPTION_GROUP
        && e->children) {
        ZapList *ch = (ZapList *)e->children;
        for (int64_t i = 0; i < ch->len; i++) {
            void *child = (void *)(intptr_t)ch->data[i];
            zap_list_push_ptr(out, zap_exception_message(child));
        }
    } else {
        zap_list_push_ptr(out, zap_exception_message(exc));
    }
    return out;
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
        /* A recorded child failure would outrank the deadline; with none, the
         * scope's own deadline surfaces as `ScopeTimeout` to the caller (I#475).
         * Otherwise an enclosing scope may have cancelled us -> `Cancelled`. */
        if (sc->timed_out) zap_raise(scope_timeout_exc(sc));
        zap_check_cancel();
        return;
    }
    if (sc->timed_out) scope_discard_timeout_msg(sc); /* child failure outranks it */
    if (sc->nexc == 1) {
        zap_raise(sc->excs[0]);
    }
    zap_raise(build_exception_group(sc->excs, sc->nexc));
}

/* Early-exit/failure join (D-0044 rule 4): the parent block is leaving with its
 * own in-flight exception, so cancel the owned coroutines, await them, and
 * return — the generated landing pad then re-raises the parent's exception. Any
 * failures the children recorded are discarded (the parent's exception wins). */
void zap_scope_exit_cancel(void *scope_) {
    Scope *sc = (Scope *)scope_;
    scope_request_cancel(sc);
    scope_join(sc);
    zap_scope_disarm(sc); /* drop any cancel_on subscription (ticket #77) */
}

/* The exception landing-pad variant: cancel + await children exactly like
 * `zap_scope_exit_cancel`, then decide which exception the caller sees and return
 * it for the generated pad to re-raise (I#475). Precedence (reporting only):
 *   1. a recorded child failure outranks a block-level `ScopeTimeout`/`Cancelled`
 *      (so a failing child is never masked by the cancel it triggered);
 *   2. otherwise, if THIS scope's own deadline fired, the in-flight `Cancelled`
 *      the body received is replaced by `ScopeTimeout` (the caller is outside the
 *      scope and should learn it timed out, not that it was cancelled);
 *   3. otherwise the body's own exception wins — a genuine error, or a `Cancelled`
 *      injected by an *enclosing* scope, which must propagate upward unchanged. */
void *zap_scope_exit_cancel_exc(void *scope_, void *pending) {
    Scope *sc = (Scope *)scope_;
    scope_request_cancel(sc);
    scope_join(sc);
    zap_scope_disarm(sc);
    int pending_is_cancel = zap_exception_is_a(pending, ZAP_EXC_CANCELLED);
    if (pending_is_cancel && sc->nexc > 0) {
        if (sc->timed_out) scope_discard_timeout_msg(sc); /* child failure outranks */
        if (sc->nexc == 1) return sc->excs[0];
        return build_exception_group(sc->excs, sc->nexc);
    }
    if (pending_is_cancel && sc->timed_out) {
        return scope_timeout_exc(sc);
    }
    if (sc->timed_out) scope_discard_timeout_msg(sc); /* body's own exception wins */
    return pending;
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
    /* #825: a severed orphan (a non-draining child detached by a `scope(timeout=)`
     * deadline) is joined by nobody and its result is discarded. Dropping its
     * uncaught exception — above all the `Cancelled` it may still observe, or any
     * genuine error — is deliberate: re-aborting the process here is exactly the
     * whole-process kill this fix removes (the caller already caught `ScopeTimeout`
     * and the program is meant to keep running). A plain `spawn detached`
     * coroutine (severed == 0) keeps the Go abort-on-uncaught rule below. */
    if (g && g->severed) {
        zap_eh_end_catch();
        return;
    }
    if (g && g->scope) {
        Scope *sc = g->scope;
        if (exc == (void *)cancelled_exc()) {
            zap_eh_end_catch();
            return; /* a cooperatively-cancelled sibling: not a failure */
        }
        zap_np_lock(&sc->mu);
        if (sc->nexc == sc->exc_cap) {
            int64_t ncap = sc->exc_cap ? sc->exc_cap * 2 : 4;
            void **nx = (void **)zap_ptrbuf((int64_t)ncap * (int64_t)sizeof(void *));
            if (sc->excs) {
                memcpy(nx, sc->excs, (size_t)sc->nexc * sizeof(void *));
            }
            sc->excs = nx;
            sc->exc_cap = ncap;
        }
        sc->excs[sc->nexc++] = exc;
        int cancel_siblings = sc->on_error == 0;
        zap_np_unlock(&sc->mu);
        if (cancel_siblings) scope_request_cancel(sc);
        zap_eh_end_catch();
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

/* deque element kinds (#983): the ring stores element words; a `PTR` deque holds
 * GC pointers (traced precisely by the ZAP_TD_DEQUE arm), a `F64` deque holds
 * reinterpreted doubles (non-pointer words), an `I64` deque plain integers. The
 * struct is declared here — before the precise-collector include — so the tracer
 * can see its layout; the functions and descriptor are defined further below. */
enum { DEQUE_ELEM_I64 = 0, DEQUE_ELEM_PTR = 1, DEQUE_ELEM_F64 = 2 };
typedef struct {
    int64_t *data;   /* `cap` element words, ring-indexed; NULL until first grow */
    int64_t cap;     /* allocated slots (fixed = maxlen when bounded)            */
    int64_t head;    /* ring index of the leftmost element                      */
    int64_t len;     /* live element count                                      */
    int64_t maxlen;  /* bound, or -1 when unbounded                             */
    int64_t kind;    /* DEQUE_ELEM_I64 | DEQUE_ELEM_PTR | DEQUE_ELEM_F64         */
} ZapDeque;

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
static void coready(Waiter *w) {
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
 * return; the waiter's outcome has been written by the waker before `coready`.
 * A coroutine parks its G and frees its M (the scheduler unlocks `mu` once the G
 * is fully switched out, which closes the park/wake race); a non-coroutine
 * (main) thread blocks on the waiter's own condvar without consuming a worker M.
 *
 * This is the one shared park primitive the whole runtime blocks on — channels
 * (`chan_block`), scope joins, and the netpoller (ticket #80) all reuse it, so a
 * blocked coroutine is always parked rather than tying up an M. */
static void copark(pthread_mutex_t *mu, Waiter *w) {
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
        g_enter_switch_out(g); /* I#889: non-preemptible from here to the swap */
        w->g = g;
        g->state = G_PARKED;
        g->parkmu = mu; /* scheduler unlocks once we are fully parked */
        /* Parking ends this coroutine's current runtime critical section: the
         * scheduler releases `mu` on our behalf, and clears the no-preempt depth
         * (ticket #172) itself once the swap has returned (`switch_to_g`) rather
         * than carry it past the switch — clearing it HERE would reopen the
         * state-store -> swap window (I#889). */
        zap_ctx_swap(&g->ctx, &m->sched_ctx);
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

/* Live channel waits, including cancellation references, stay rooted until the
 * last reference leaves. Lock order is channel -> registry. A canceller takes
 * a reference under the registry lock, drops it, then acquires the channel. */
typedef struct ScopeWait {
    struct ScopeWait *next;
    Scope *scope;
    ZapChan *channel;
    Waiter *waiter;
    int refs;
} ScopeWait;
static pthread_mutex_t g_scope_wait_mu = PTHREAD_MUTEX_INITIALIZER;
static ScopeWait *g_scope_waits;

static void scope_wait_release(ScopeWait *node) {
    if (--node->refs == 0) {
        ScopeWait **link = &g_scope_waits;
        while (*link && *link != node) link = &(*link)->next;
        if (*link) *link = node->next;
        /* GC owns the unlinked buffer. */
    }
}
static int scope_q_remove(Waiter **head, Waiter **tail, Waiter *target) {
    Waiter *previous = NULL;
    for (Waiter *w = *head; w; previous = w, w = w->next) {
        if (w != target) continue;
        if (previous) previous->next = w->next; else *head = w->next;
        if (*tail == w) *tail = previous;
        w->next = NULL;
        return 1;
    }
    return 0;
}
static int scope_contains(Scope *ancestor, Scope *scope) {
    for (; scope; scope = scope->parent) if (scope == ancestor) return 1;
    return 0;
}
static void scope_cancel_io(Scope *sc);

static void scope_request_cancel(Scope *sc) {
    zap_np_lock(&sc->mu);
    sc->cancelled = 1;
    pthread_cond_broadcast(&sc->cv);
    zap_np_unlock(&sc->mu);
    scope_cancel_io(sc);
    zap_np_lock(&g_scope_wait_mu);
    size_t count = 0;
    for (ScopeWait *n = g_scope_waits; n; n = n->next)
        if (n->waiter && scope_contains(sc, n->scope)) count++;
    ScopeWait **nodes = count ? malloc(count * sizeof(*nodes)) : NULL;
    if (count && !nodes) abort();
    size_t i = 0;
    for (ScopeWait *n = g_scope_waits; n; n = n->next) {
        if (n->waiter && scope_contains(sc, n->scope)) { n->refs++; nodes[i++] = n; }
    }
    zap_np_unlock(&g_scope_wait_mu);
    for (i = 0; i < count; i++) {
        ScopeWait *n = nodes[i];
        ZapChan *c = n->channel;
        zap_np_lock(&c->mu);
        zap_np_lock(&g_scope_wait_mu);
        Waiter *w = n->waiter;
        if (w && (scope_q_remove(&c->sendq_head, &c->sendq_tail, w) ||
                  scope_q_remove(&c->recvq_head, &c->recvq_tail, w))) {
            w->ok = 0;
            coready(w);
        }
        scope_wait_release(n);
        zap_np_unlock(&g_scope_wait_mu);
        zap_np_unlock(&c->mu);
    }
    free(nodes);
}

static void chan_block(ZapChan *c, Waiter *w) {
    G *g = cur_m ? cur_m->current : NULL;
    if (g) g->park_reason = "channel"; /* I#475 diagnostics */
    ScopeWait *node = NULL;
    if (g && g->scope) {
        node = (ScopeWait *)zap_ptrbuf(sizeof(ScopeWait));
        node->scope = g->scope; node->channel = c; node->waiter = w; node->refs = 1;
        zap_np_lock(&g_scope_wait_mu);
        node->next = g_scope_waits; g_scope_waits = node;
        int cancelled = g_is_cancelled();
        if (cancelled) {
            scope_q_remove(&c->sendq_head, &c->sendq_tail, w);
            scope_q_remove(&c->recvq_head, &c->recvq_tail, w);
            node->waiter = NULL; scope_wait_release(node);
            zap_np_unlock(&g_scope_wait_mu);
            zap_np_unlock(&c->mu);
            zap_raise(cancelled_exc());
            return;
        }
        zap_np_unlock(&g_scope_wait_mu);
    }
    copark(&c->mu, w);
    if (node) {
        zap_np_lock(&c->mu);
        zap_np_lock(&g_scope_wait_mu);
        node->waiter = NULL; scope_wait_release(node);
        zap_np_unlock(&g_scope_wait_mu);
        zap_np_unlock(&c->mu);
    }
    zap_check_cancel();
}
#ifdef ZAP_GC_IMMIX
static void zap_scope_wait_mark_roots(void) {
    zap_np_lock(&g_scope_wait_mu);
    for (ScopeWait *n = g_scope_waits; n; n = n->next) {
        zap_gc_mark(n); zap_gc_mark(n->channel); zap_gc_mark(n->scope);
    }
    zap_np_unlock(&g_scope_wait_mu);
}
#endif

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

/* --- `select` (D-0031, #1002): one coroutine waiting on several channel ops.
 *
 * A `select` enqueues one per-case waiter (`w->sel == sel`, `w->sel_case == i`)
 * on each case's channel queue and parks ONCE on `sel->pw` via `copark(&sel->mu,
 * sel->pw)`. A counterpart on any one channel that pops such a waiter must CLAIM
 * the select first: lock `sel->mu`, and if `sel->fired` is already set the
 * waiter is a dead case from a select that completed on another channel — skip
 * it and keep popping; otherwise set `fired`, record the chosen case + outcome,
 * and wake `sel->pw`. Because `sel->mu` is the park mutex, the scheduler only
 * releases it after the selecting G has fully switched out, so a counterpart
 * cannot wake the select before it is parked (the same handoff invariant plain
 * channel ops get from `copark`). Lock order is always channel-mu then sel-mu. */
typedef struct SelState {
    pthread_mutex_t mu;
    int fired;        /* 0 until a case completes; claimed (0->1) under `mu` */
    int chosen;       /* the completed case index */
    uint64_t rval;    /* receive case: the delivered word */
    int rok;          /* receive: real value (1) vs closed (0); send: 1 = sent */
    int send_closed;  /* a send case fired because the channel was closed */
    Waiter *pw;       /* the parking waiter (copark vehicle; carries the G) */
} SelState;

/* Deliver value word `v` to a receiver waiter `r` popped from a recvq. Returns
 * 1 if `r` was woken (the send is complete), 0 if `r` was a dead `select` case
 * (claimed elsewhere) and the caller should discard it and keep popping. */
static int deliver_to_receiver(Waiter *r, uint64_t v, void *from) {
    if (r->sel) {
        SelState *sel = r->sel;
        pthread_mutex_lock(&sel->mu);
        if (sel->fired) {
            pthread_mutex_unlock(&sel->mu);
            return 0;
        }
        sel->fired = 1;
        sel->chosen = r->sel_case;
        sel->rval = v;
        sel->rok = 1;
        zap_race_chan_xfer(from, sel->pw->g);
        coready(sel->pw);
        pthread_mutex_unlock(&sel->mu);
        return 1;
    }
    r->val = v;
    r->ok = 1;
    zap_race_chan_xfer(from, r->g);
    coready(r);
    return 1;
}

/* Take sender waiter `s`'s offered value into `*out` and wake it. Returns 1 if
 * taken (recv complete), 0 if `s` was a dead `select` case (discard, keep
 * popping). `to` is the receiving G for the --race rendezvous edge. */
static int take_from_sender(Waiter *s, uint64_t *out, void *to) {
    if (s->sel) {
        SelState *sel = s->sel;
        pthread_mutex_lock(&sel->mu);
        if (sel->fired) {
            pthread_mutex_unlock(&sel->mu);
            return 0;
        }
        sel->fired = 1;
        sel->chosen = s->sel_case;
        sel->rok = 1; /* the send completed */
        *out = s->val;
        zap_race_chan_xfer(sel->pw->g, to);
        coready(sel->pw);
        pthread_mutex_unlock(&sel->mu);
        return 1;
    }
    *out = s->val;
    s->ok = 1;
    zap_race_chan_xfer(s->g, to);
    coready(s);
    return 1;
}

/* After a receiver drains one buffered slot, move a parked sender's value into
 * the freed slot and wake it (skipping dead `select` send-cases). Call with the
 * channel locked and a slot just freed (`count < cap`). Shared by the buffered
 * receive path and the `select` buffered-receive commit. */
static void chan_buf_refill_from_sender(ZapChan *c) {
    Waiter *s;
    while ((s = q_pop(&c->sendq_head, &c->sendq_tail)) != NULL) {
        SelState *sel = s->sel;
        if (sel) {
            pthread_mutex_lock(&sel->mu);
            if (sel->fired) {
                pthread_mutex_unlock(&sel->mu);
                continue;
            }
            sel->fired = 1;
            sel->chosen = s->sel_case;
            sel->rok = 1;
        }
        int64_t tail_i = c->tail;
        c->buf[tail_i] = s->val;
        c->tail = (c->tail + 1) % c->cap;
        c->count++;
        /* --race: the refilled slot now carries the parked sender's value;
         * stamp it with THAT sender's clock for its eventual receiver. */
        zap_race_chan_buf_store(&c->buf[tail_i], sel ? sel->pw->g : s->g);
        if (sel) {
            coready(sel->pw);
            pthread_mutex_unlock(&sel->mu);
        } else {
            s->ok = 1;
            coready(s);
        }
        return;
    }
}

static void chan_send_word(ZapChan *c, uint64_t v) {
    /* A channel op is a cancellation point (design/0005): if this coroutine's
     * scope was cancelled, raise `Cancelled` rather than proceeding/blocking. */
    zap_check_cancel();
    zap_np_lock(&c->mu);
    if (c->closed) {
        zap_np_unlock(&c->mu);
        zap_panic(zap_str_new("send on closed channel", 22));
        return;
    }
    /* A waiting receiver takes the value directly (rendezvous / fast path). The
     * --race hand-off edge (this sender happens-before the receiver, published
     * before the wake so it is in place before the receiver resumes on another
     * M, #679) lives in `deliver_to_receiver`, which also skips any dead
     * `select` receive-case already claimed on another channel (D-0031). */
    if (c->count == 0) {
        Waiter *r;
        while ((r = q_pop(&c->recvq_head, &c->recvq_tail)) != NULL) {
            if (deliver_to_receiver(r, v, cur_m ? cur_m->current : NULL)) {
                zap_np_unlock(&c->mu);
                return;
            }
        }
    }
    if (c->cap > 0 && c->count < c->cap) {
        int64_t slot_i = c->tail;
        c->buf[slot_i] = v;
        c->tail = (c->tail + 1) % c->cap;
        c->count++;
        /* --race: buffered produce — stamp the slot with this sender's clock;
         * the receiver that later drains the slot acquires it (#679). */
        zap_race_chan_buf_store(&c->buf[slot_i], cur_m ? cur_m->current : NULL);
        zap_np_unlock(&c->mu);
        return;
    }
    /* Would block: unbuffered with no receiver, or buffer full. Park as a
     * sender; the matching receiver delivers/buffers our value and wakes us. */
    Waiter w = {0};
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
    zap_np_lock(&c->mu);
    /* Buffered value available: take it, then let a waiting sender refill. */
    if (c->count > 0) {
        int64_t head_i = c->head;
        uint64_t v = c->buf[head_i];
        c->head = (c->head + 1) % c->cap;
        c->count--;
        /* --race: buffered consume — acquire the clock the producer stamped on
         * this slot, ordering this receiver after that send (#679). */
        zap_race_chan_buf_load(&c->buf[head_i], cur_m ? cur_m->current : NULL);
        chan_buf_refill_from_sender(c);
        zap_np_unlock(&c->mu);
        if (ok) {
            *ok = 1;
        }
        return v;
    }
    /* A waiting sender hands its value over directly (rendezvous); skip dead
     * `select` send-cases. The --race hand-off edge (sender happens-before this
     * receiver, #679) lives in `take_from_sender`. */
    {
        Waiter *s;
        while ((s = q_pop(&c->sendq_head, &c->sendq_tail)) != NULL) {
            uint64_t v;
            if (take_from_sender(s, &v, cur_m ? cur_m->current : NULL)) {
                zap_np_unlock(&c->mu);
                if (ok) {
                    *ok = 1;
                }
                return v;
            }
        }
    }
    if (c->closed) {
        zap_np_unlock(&c->mu);
        if (ok) {
            *ok = 0;
        }
        return 0;
    }
    /* Would block: park as a receiver; a sender (or close) wakes us. */
    Waiter w = {0};
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
/* `channel[float64]` (#800): the element travels as a reinterpreted `double`
 * word. It is not a GC pointer, so `elem_ptr` stays 0 and the collector never
 * follows a buffered/in-flight float element. */
void zap_chan_send_f64(void *ch, double v) {
    chan_send_word((ZapChan *)ch, (uint64_t)zap_f64_bits(v));
}
double zap_chan_recv_f64(void *ch) {
    int ok;
    return zap_f64_from_bits((int64_t)chan_recv_word((ZapChan *)ch, &ok));
}

void zap_chan_close(void *ch) {
    ZapChan *c = (ZapChan *)ch;
    zap_np_lock(&c->mu);
    c->closed = 1;
    /* Waiting senders fail (send on closed); waiting receivers drain to the
     * closed zero-value with ok == 0. A `select` case is claimed first (and a
     * dead one — already fired elsewhere — is simply dropped, D-0031). */
    Waiter *w;
    while ((w = q_pop(&c->sendq_head, &c->sendq_tail)) != NULL) {
        if (w->sel) {
            SelState *sel = w->sel;
            pthread_mutex_lock(&sel->mu);
            if (!sel->fired) {
                sel->fired = 1;
                sel->chosen = w->sel_case;
                sel->send_closed = 1; /* send case on a closed channel: raises */
                coready(sel->pw);
            }
            pthread_mutex_unlock(&sel->mu);
            continue;
        }
        w->closed = 1;
        w->ok = 0;
        coready(w);
    }
    while ((w = q_pop(&c->recvq_head, &c->recvq_tail)) != NULL) {
        if (w->sel) {
            SelState *sel = w->sel;
            pthread_mutex_lock(&sel->mu);
            if (!sel->fired) {
                sel->fired = 1;
                sel->chosen = w->sel_case;
                sel->rval = 0;
                sel->rok = 0; /* closed receive: comma-ok false */
                coready(sel->pw);
            }
            pthread_mutex_unlock(&sel->mu);
            continue;
        }
        w->ok = 0;
        w->val = 0;
        coready(w);
    }
    zap_np_unlock(&c->mu);
}

/* === `select` statement runtime (D-0031, #1002) ===========================
 *
 * `zap_select` waits on several channel operations and proceeds with the first
 * that is ready, choosing uniformly at random among several ready cases
 * (fairness, like Go). A `default` case (`has_default`) makes it non-blocking.
 *
 * The cases are passed as direct, per-call-site-fixed variadic arguments — n
 * triples `(void* ch, int64_t dir, uint64_t sendval)`, `dir` bit0 = send,
 * bit1 = element-is-pointer — so a pointer send value stays rooted by the
 * COMPILED caller across the park, exactly as a plain `zap_chan_send_ptr(v)`
 * argument does. For the chosen case, `*outval`/`*outok` carry a receive's
 * delivered word + comma-ok flag (loaded straight-line by the caller, with no
 * intervening park). Returns the chosen case index, or -1 for `default`. */
typedef struct {
    ZapChan *ch;
    int is_send;
    uint64_t sendval;
} ZapSelCase;

/* Cheap xorshift PRNG for case fairness (scheduling, not security). The state
 * is a caller-owned stack local — never a `static`/`__thread`, which a migrating
 * coroutine could carry to the wrong OS thread (I#891). */
static uint64_t sel_rng_next(uint64_t *state) {
    uint64_t s = *state;
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    *state = s;
    return s;
}

/* Try to commit a receive on `c` (channel locked). Returns 1 and sets
 * *rval/*rok on success; 0 if not ready. Mirrors `chan_recv_word`'s ready
 * paths (skipping dead select senders). */
static int sel_try_recv(ZapChan *c, uint64_t *rval, int *rok, void *to) {
    if (c->count > 0) {
        int64_t h = c->head;
        uint64_t v = c->buf[h];
        c->head = (c->head + 1) % c->cap;
        c->count--;
        zap_race_chan_buf_load(&c->buf[h], to);
        chan_buf_refill_from_sender(c);
        *rval = v;
        *rok = 1;
        return 1;
    }
    Waiter *s;
    while ((s = q_pop(&c->sendq_head, &c->sendq_tail)) != NULL) {
        uint64_t v;
        if (take_from_sender(s, &v, to)) {
            *rval = v;
            *rok = 1;
            return 1;
        }
    }
    if (c->closed) {
        *rval = 0;
        *rok = 0;
        return 1;
    }
    return 0;
}

/* Try to commit a send of `v` on `c` (channel locked). Returns 1 on success,
 * -1 if the channel is closed (the caller raises), 0 if not ready. */
static int sel_try_send(ZapChan *c, uint64_t v, void *from) {
    if (c->closed) {
        return -1;
    }
    Waiter *r;
    while ((r = q_pop(&c->recvq_head, &c->recvq_tail)) != NULL) {
        if (deliver_to_receiver(r, v, from)) {
            return 1;
        }
    }
    if (c->cap > 0 && c->count < c->cap) {
        int64_t t = c->tail;
        c->buf[t] = v;
        c->tail = (c->tail + 1) % c->cap;
        c->count++;
        zap_race_chan_buf_store(&c->buf[t], from);
        return 1;
    }
    return 0;
}

int64_t zap_select(uint64_t *outval, int64_t *outok, int64_t n, int64_t has_default, ...) {
    zap_check_cancel(); /* a select is a cancellation point (design/0005) */
    int64_t cap = n > 0 ? n : 1;
    ZapSelCase cases[cap];
    va_list ap;
    va_start(ap, has_default);
    for (int64_t i = 0; i < n; i++) {
        cases[i].ch = (ZapChan *)va_arg(ap, void *);
        int64_t dir = va_arg(ap, int64_t);
        cases[i].is_send = (int)(dir & 1);
        cases[i].sendval = (uint64_t)va_arg(ap, uint64_t);
        if ((dir >> 1) & 1) {
            /* A pointer-element channel: ensure the collector traces buffered
             * elements (the typed `_ptr` entry points set this; `zap_select`
             * is untyped, so the caller flags it in `dir`). */
            cases[i].ch->elem_ptr = 1;
        }
    }
    va_end(ap);

    void *from = cur_m ? (void *)cur_m->current : NULL;

    /* Distinct channels, address-sorted: a stable lock order (no deadlock when
     * two selects share channels). Insertion sort — `n` is small. */
    ZapChan *locks[cap];
    int nl = 0;
    for (int64_t i = 0; i < n; i++) {
        ZapChan *c = cases[i].ch;
        int seen = 0;
        for (int j = 0; j < nl; j++) {
            if (locks[j] == c) {
                seen = 1;
                break;
            }
        }
        if (!seen) {
            locks[nl++] = c;
        }
    }
    for (int a = 1; a < nl; a++) {
        ZapChan *k = locks[a];
        int b = a - 1;
        while (b >= 0 && (uintptr_t)locks[b] > (uintptr_t)k) {
            locks[b + 1] = locks[b];
            b--;
        }
        locks[b + 1] = k;
    }
    for (int j = 0; j < nl; j++) {
        zap_np_lock(&locks[j]->mu);
    }

    /* Pass 1: scan the cases in a shuffled order and take the first ready one
     * (uniform fairness among ready cases). */
    int order[cap];
    for (int64_t i = 0; i < n; i++) {
        order[i] = (int)i;
    }
    /* Seed the shuffle from stack + coroutine identity — varied per call, no
     * static state to migrate across OS threads (I#891). */
    uint64_t rng = ((uint64_t)(uintptr_t)order ^ ((uint64_t)(uintptr_t)from * 0x9e3779b97f4a7c15ull)) | 1;
    for (int64_t i = n; i > 1;) {
        i--;
        int j = (int)(sel_rng_next(&rng) % (uint64_t)(i + 1));
        int t = order[i];
        order[i] = order[j];
        order[j] = t;
    }

    int chosen = -1, send_closed = 0, rok = 0;
    uint64_t rval = 0;
    for (int64_t k = 0; k < n; k++) {
        int i = order[k];
        ZapSelCase *sc = &cases[i];
        if (sc->is_send) {
            int r = sel_try_send(sc->ch, sc->sendval, from);
            if (r == 1) {
                chosen = i;
                rok = 1;
                break;
            }
            if (r == -1) {
                chosen = i;
                send_closed = 1;
                break;
            }
        } else if (sel_try_recv(sc->ch, &rval, &rok, from)) {
            chosen = i;
            break;
        }
    }

    if (chosen >= 0) {
        for (int j = nl - 1; j >= 0; j--) {
            zap_np_unlock(&locks[j]->mu);
        }
        if (send_closed) {
            zap_panic(zap_str_new("send on closed channel", 22));
        }
        if (outval) {
            *outval = rval;
        }
        if (outok) {
            *outok = rok;
        }
        return chosen;
    }
    if (has_default) {
        for (int j = nl - 1; j >= 0; j--) {
            zap_np_unlock(&locks[j]->mu);
        }
        return -1;
    }

    /* Pass 2: no case ready and no default — enqueue one waiter per case and
     * park once. `sel.mu` is the park mutex: a counterpart that pops one of our
     * waiters must take `sel.mu` to claim the select, and the scheduler releases
     * `sel.mu` only after we have fully switched out, so we cannot be woken
     * before we are parked (the `copark` handoff invariant, now across many
     * channels). */
    SelState sel;
    pthread_mutex_init(&sel.mu, NULL);
    sel.fired = 0;
    sel.chosen = -1;
    sel.rval = 0;
    sel.rok = 0;
    sel.send_closed = 0;
    Waiter pw = {0};
    sel.pw = &pw;
    Waiter wcases[cap];
    for (int64_t i = 0; i < n; i++) {
        Waiter *w = &wcases[i];
        *w = (Waiter){0};
        w->sel = &sel;
        w->sel_case = (int)i;
        w->val = cases[i].sendval;
        if (cases[i].is_send) {
            q_push(&cases[i].ch->sendq_head, &cases[i].ch->sendq_tail, w);
        } else {
            q_push(&cases[i].ch->recvq_head, &cases[i].ch->recvq_tail, w);
        }
    }
    pthread_mutex_lock(&sel.mu);
    for (int j = nl - 1; j >= 0; j--) {
        zap_np_unlock(&locks[j]->mu);
    }
    copark(&sel.mu, &pw); /* releases sel.mu once fully parked */

    /* Resumed: one case fired (sel.chosen/.rval/.rok/.send_closed are set).
     * Unlink our remaining waiters from every other channel. A concurrent
     * counterpart that pops a stale waiter sees `sel.fired` and discards it, so
     * a double-removal here is harmless. */
    for (int64_t i = 0; i < n; i++) {
        if ((int)i == sel.chosen) {
            continue;
        }
        ZapChan *c = cases[i].ch;
        zap_np_lock(&c->mu);
        scope_q_remove(&c->sendq_head, &c->sendq_tail, &wcases[i]);
        scope_q_remove(&c->recvq_head, &c->recvq_tail, &wcases[i]);
        zap_np_unlock(&c->mu);
    }
    pthread_mutex_destroy(&sel.mu);
    zap_check_cancel(); /* a cancel may have severed us while parked */
    if (sel.send_closed) {
        zap_panic(zap_str_new("send on closed channel", 22));
    }
    if (outval) {
        *outval = sel.rval;
    }
    if (outok) {
        *outok = sel.rok;
    }
    return sel.chosen;
}

/* ===========================================================================
 * Netpoller — non-blocking I/O readiness for the M:N scheduler
 * (ticket #80, D-0008, design/0005-concurrency.md "Blocking syscalls").
 *
 * A coroutine that would block waiting for a file descriptor to become readable
 * or writable registers the fd here and PARKS its G (freeing its M to run other
 * coroutines) instead of blocking the OS thread in a read/write syscall. A
 * dedicated poller OS thread waits on the readiness mechanism (epoll on Linux)
 * and, when an fd is ready, re-queues the parked G on the scheduler. This is the
 * Go netpoller analogue and the deferral called out in #47's PR.
 *
 * It reuses the scheduler's ONE park/wake primitive (`copark` / `coready`),
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
enum { ZAP_POLL_OK = 0, ZAP_POLL_TIMEOUT = 1, ZAP_POLL_CLOSED = 2, ZAP_POLL_CANCELLED = 3 };

typedef struct PollDesc {
    int fd;
    int closing;                 /* zap_netpoll_close has been called           */
    pthread_mutex_t mu;          /* guards the waiter slots / latches / deadlines */
    Waiter *rg;                  /* parked reader waiter, or NULL                */
    Waiter *wg;                  /* parked writer waiter, or NULL                */
    /* Borrowed owning scopes: live parked Gs root them. Cleared BEFORE waking,
     * under mu, so cancellation never retains a waiter or a recycled pd. */
    Scope *r_scope;
    Scope *w_scope;
    int64_t io_deadline_ns; /* persistent absolute deadline for both directions */
    int r_ready;                 /* readiness edge latched with no reader parked  */
    int w_ready;
    int64_t r_deadline;          /* CLOCK_MONOTONIC ns, or 0 = none              */
    int64_t w_deadline;
    struct PollDesc *all_prev;   /* live-desc registry (deadline scan)           */
    struct PollDesc *all_next;
    struct PollDesc *free_link;  /* free list when idle                          */
#if defined(ZAP_PLATFORM_WINDOWS)
    /* IOCP backend (I#196 / A2): the socket handle (the ABI's int64_t fd carries a
     * Windows SOCKET, which the `int fd` above cannot hold — Q3) and the opaque
     * zap_np_iocp.c descriptor. Windows-only, #ifdef-guarded so the Linux struct
     * layout is byte-for-byte unchanged. */
    uint64_t sock;
    void *afd;
#endif
} PollDesc;

/* Registry + free list + lifecycle lock. Lock order is ALWAYS g_np_mu then a
 * PollDesc's own mu; no path takes them the other way round. */
static pthread_mutex_t g_np_mu = PTHREAD_MUTEX_INITIALIZER;
static PollDesc *g_np_all = NULL;   /* doubly-linked list of live descriptors   */
static PollDesc *g_np_free = NULL;  /* recycled descriptors                      */

static pthread_once_t g_np_once = PTHREAD_ONCE_INIT;
static volatile int g_np_started = 0;
/* Poller-thread exit latch (#213, D-0121). The shutdown thread sets it then
 * kicks the poller off its wait and joins; the poller reads it after each wait.
 * Accessed via RELAXED `__atomic_*` (not plain `volatile`) for a well-defined
 * cross-thread load. Relaxed is enough: no payload travels with it, and
 * liveness comes from the kick plus the poller's bounded re-poll, not from this
 * flag's ordering. */
static int g_np_shutdown = 0;
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
#elif defined(ZAP_PLATFORM_WINDOWS)
#define ZAP_NETPOLL_IOCP 1
#include "zap_np_iocp.h"

/* IOCP backend (I#196 / A2; design/0020). The readiness-oriented seam is bridged
 * to IOCP's completion model by zap_np_iocp.c's AFD-poll layer, WITHOUT widening
 * the seam or touching the epoll arm. `np_backend_add` registers the socket (and
 * the owning PollDesc as the completion userkey); arming is lazy and gated in the
 * Windows np_poller_main (Q1), so there is no edge-triggered "register once"
 * EPOLLET analogue to do here. `np_backend_kick` posts a sentinel completion. */
static int np_backend_open(void) { return zap_iocp_global_open(); }

static int np_backend_add(PollDesc *pd) {
    pd->afd = zap_iocp_register(pd->sock, pd);
    if (!pd->afd) {
        fprintf(stderr, "zap: netpoller could not register socket (IOCP/AFD err %d)\n",
                zap_iocp_last_error());
        return -1;
    }
    return 0;
}

static void np_backend_del(PollDesc *pd) {
    if (pd->afd) {
        zap_iocp_unregister(pd->afd);
        pd->afd = NULL;
    }
}

static void np_backend_kick(void) { zap_iocp_kick(); }
#else
/* No readiness backend on this platform (kqueue follow-up): the netpoller
 * degrades to "unsupported" — zap_netpoll_open returns NULL. */
static int g_np_epfd = -1;
static int np_backend_open(void) { return -1; }
static int np_backend_add(PollDesc *pd) { (void)pd; return -1; }
static void np_backend_del(PollDesc *pd) { (void)pd; }
static void np_backend_kick(void) {}
#endif

/* ---- generic poller (backend-agnostic) ------------------------------------ */
/* Deliver readiness/timeout/closed to one direction of `pd` (pd->mu held). A
 * parked waiter is popped, given its outcome, and re-queued (`coready`); a bare
 * readiness edge with no waiter is LATCHED so the next `wait` consumes it. */
static void np_wake_dir_locked(PollDesc *pd, int is_read, int status) {
    Waiter **slot = is_read ? &pd->rg : &pd->wg;
    int *ready = is_read ? &pd->r_ready : &pd->w_ready;
    int64_t *dl = is_read ? &pd->r_deadline : &pd->w_deadline;
    if (*slot) {
        Waiter *w = *slot;
        *slot = NULL;
        if (is_read) pd->r_scope = NULL; else pd->w_scope = NULL;
        *dl = 0;
        w->val = (uint64_t)status;
        coready(w); /* enqueue the parked G, or signal a native waiter's condvar */
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
    zap_np_lock(&g_np_mu);
    for (PollDesc *pd = g_np_all; pd; pd = pd->all_next) {
        zap_np_lock(&pd->mu);
        int64_t d = 0;
        if (pd->rg && pd->r_deadline) {
            d = pd->r_deadline;
        }
        if (pd->wg && pd->w_deadline && (d == 0 || pd->w_deadline < d)) {
            d = pd->w_deadline;
        }
        zap_np_unlock(&pd->mu);
        if (d && (!have || d < best)) {
            best = d;
            have = 1;
        }
    }
    zap_np_unlock(&g_np_mu);
    if (!have) {
        return -1;
    }
    int64_t now = np_now_ns();
    if (best <= now) {
        return 0;
    }
    int64_t ms = 1 + (best - now - 1) / 1000000;
    if (ms > 3600000) {
        ms = 3600000;
    }
    return (int)ms;
}

/* Wake every waiter whose deadline has passed (called after each poll wait). */
static void np_expire_deadlines(void) {
    int64_t now = np_now_ns();
    zap_np_lock(&g_np_mu);
    for (PollDesc *pd = g_np_all; pd; pd = pd->all_next) {
        zap_np_lock(&pd->mu);
        if (pd->rg && pd->r_deadline && pd->r_deadline <= now) {
            Waiter *w = pd->rg;
            pd->rg = NULL;
            pd->r_scope = NULL;
            pd->r_deadline = 0;
            w->val = ZAP_POLL_TIMEOUT;
            coready(w);
        }
        if (pd->wg && pd->w_deadline && pd->w_deadline <= now) {
            Waiter *w = pd->wg;
            pd->wg = NULL;
            pd->w_scope = NULL;
            pd->w_deadline = 0;
            w->val = ZAP_POLL_TIMEOUT;
            coready(w);
        }
        zap_np_unlock(&pd->mu);
    }
    zap_np_unlock(&g_np_mu);
}

#if defined(ZAP_NETPOLL_EPOLL)
static void *np_poller_main(void *arg) {
    (void)arg;
    struct epoll_event evs[64];
    for (;;) {
        int timeout = np_compute_timeout();
        int n = epoll_wait(g_np_epfd, evs, 64, timeout);
        if (__atomic_load_n(&g_np_shutdown, __ATOMIC_RELAXED)) {
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
            zap_np_lock(&pd->mu);
            /* An error/hangup counts as readiness for BOTH directions so the
             * parked G wakes and observes the condition via its next syscall. */
            if (e & (EPOLLIN | EPOLLERR | EPOLLHUP | EPOLLRDHUP)) {
                np_wake_dir_locked(pd, 1, ZAP_POLL_OK);
            }
            if (e & (EPOLLOUT | EPOLLERR | EPOLLHUP)) {
                np_wake_dir_locked(pd, 0, ZAP_POLL_OK);
            }
            zap_np_unlock(&pd->mu);
        }
        np_expire_deadlines();
    }
    return NULL;
}
#endif

#if defined(ZAP_NETPOLL_IOCP)
/* Ensure each live descriptor has a one-shot AFD poll in flight for every
 * direction that still wants readiness, GATED so a latched-but-undrained socket
 * is not re-polled (I#196 Q1): arm READ while a reader is parked (pd->rg) or no
 * read edge is latched (!pd->r_ready), and symmetrically for WRITE. A direction
 * already armed is left alone (zap_iocp_arm is a no-op then); a latched edge with
 * no waiter leaves that direction unarmed, so it cannot busy-spin. Runs each
 * poller iteration (same cadence/cost as np_compute_timeout's registry scan). */
static void np_iocp_arm_needed(void) {
    zap_np_lock(&g_np_mu);
    for (PollDesc *pd = g_np_all; pd; pd = pd->all_next) {
        if (!pd->afd) {
            continue;
        }
        zap_np_lock(&pd->mu);
        int want_read = (pd->rg != NULL) || (pd->r_ready == 0);
        int want_write = (pd->wg != NULL) || (pd->w_ready == 0);
        if (want_read) {
            zap_iocp_arm(pd->afd, ZAP_IOCP_READ);
        }
        if (want_write) {
            zap_iocp_arm(pd->afd, ZAP_IOCP_WRITE);
        }
        zap_np_unlock(&pd->mu);
    }
    zap_np_unlock(&g_np_mu);
}

static void *np_poller_main(void *arg) {
    (void)arg;
    zap_iocp_ready readies[64];
    for (;;) {
        np_iocp_arm_needed();
        int timeout = np_compute_timeout();
        int n = zap_iocp_wait(readies, 64, timeout);
        if (__atomic_load_n(&g_np_shutdown, __ATOMIC_RELAXED)) {
            break;
        }
        if (n < 0) {
            break; /* unexpected IOCP failure: stop polling */
        }
        for (int i = 0; i < n; i++) {
            PollDesc *pd = (PollDesc *)readies[i].userkey;
            zap_np_lock(&pd->mu);
            /* An error/hangup (closed) counts as readiness for BOTH directions so
             * the parked G wakes and observes the condition via its next socket
             * call — the IOCP analogue of the epoll EPOLLERR/EPOLLHUP handling. */
            if (readies[i].readable) {
                np_wake_dir_locked(pd, 1, ZAP_POLL_OK);
            }
            if (readies[i].writable) {
                np_wake_dir_locked(pd, 0, ZAP_POLL_OK);
            }
            zap_np_unlock(&pd->mu);
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
    int rc = zap_gc_pthread_create(&g_np_thread, NULL, np_poller_main, NULL);
    if (rc != 0) {
        fprintf(stderr, "zap: netpoller thread start failed (%d)\n", rc);
        return;
    }
    g_np_started = 1;
#elif defined(ZAP_NETPOLL_IOCP)
    if (np_backend_open() != 0) {
        fprintf(stderr, "zap: netpoller init failed (IOCP err %d)\n", zap_iocp_last_error());
        return; /* g_np_started stays 0 => zap_netpoll_open reports unsupported */
    }
    int rc = zap_gc_pthread_create(&g_np_thread, NULL, np_poller_main, NULL);
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
    __atomic_store_n(&g_np_shutdown, 1, __ATOMIC_RELAXED);
    np_backend_kick();
    zap_gc_pthread_join(g_np_thread, NULL);
    g_np_started = 0;
#if defined(ZAP_NETPOLL_IOCP)
    /* The poller thread has joined, so no completion can still reference the port
     * or the AFD helper handle — release them. */
    zap_iocp_global_close();
#endif
}

/* ---- descriptor lifecycle ------------------------------------------------- */
static PollDesc *np_pd_alloc(int fd) {
    zap_np_lock(&g_np_mu);
    PollDesc *pd = g_np_free;
    if (pd) {
        g_np_free = pd->free_link;
    }
    zap_np_unlock(&g_np_mu);
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
    pd->r_scope = pd->w_scope = NULL;
    pd->io_deadline_ns = 0;
    pd->r_ready = pd->w_ready = 0;
    pd->r_deadline = pd->w_deadline = 0;
    pd->all_prev = pd->all_next = pd->free_link = NULL;
#if defined(ZAP_PLATFORM_WINDOWS)
    pd->sock = 0;
    pd->afd = NULL;
#endif
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
#if defined(ZAP_PLATFORM_WINDOWS)
    /* The ABI's int64_t fd carries a Windows SOCKET (pointer-width), which the
     * `int fd` field above truncates; the IOCP backend uses the full handle (Q3). */
    pd->sock = (uint64_t)fd;
#endif
    zap_np_lock(&g_np_mu);
    pd->all_prev = NULL;
    pd->all_next = g_np_all;
    if (g_np_all) {
        g_np_all->all_prev = pd;
    }
    g_np_all = pd;
    zap_np_unlock(&g_np_mu);
    if (np_backend_add(pd) != 0) {
        zap_np_lock(&g_np_mu);
        np_all_remove_locked(pd);
        pd->free_link = g_np_free;
        g_np_free = pd;
        zap_np_unlock(&g_np_mu);
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
    zap_np_lock(&pd->mu);
    if (g_is_cancelled()) {
        zap_np_unlock(&pd->mu);
        return ZAP_POLL_CANCELLED;
    }
    if (pd->closing) {
        zap_np_unlock(&pd->mu);
        return ZAP_POLL_CLOSED;
    }
    if (pd->io_deadline_ns && (!deadline_ns || pd->io_deadline_ns < deadline_ns))
        deadline_ns = pd->io_deadline_ns;
    if (deadline_ns && deadline_ns <= np_now_ns()) {
        zap_np_unlock(&pd->mu);
        return ZAP_POLL_TIMEOUT;
    }
    int *ready = is_read ? &pd->r_ready : &pd->w_ready;
    if (*ready) {
        *ready = 0;
        zap_np_unlock(&pd->mu);
        return ZAP_POLL_OK;
    }
    Waiter w = {0};
    w.val = ZAP_POLL_OK;
    G *g = cur_m ? cur_m->current : NULL;
    if (g) g->park_reason = "io";
    if (is_read) {
        pd->rg = &w;
        pd->r_scope = g ? g->scope : NULL;
        pd->r_deadline = deadline_ns;
    } else {
        pd->wg = &w;
        pd->w_scope = g ? g->scope : NULL;
        pd->w_deadline = deadline_ns;
    }
    /* Registration and the cancellation check share pd->mu with the canceller.
     * A cancel before registration is observed above; one after it wakes w. */
    if (deadline_ns) np_backend_kick();
    copark(&pd->mu, &w);
    /* Never touch pd after wake: close may already have recycled it. The caller
     * receives a status and releases malloc/OpenSSL scratch BEFORE raising. */
    return g_is_cancelled() ? ZAP_POLL_CANCELLED : (int64_t)w.val;
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
    zap_np_lock(&pd->mu);
    if (mode & ZAP_POLL_READ) {
        if (pd->rg) {
            Waiter *w = pd->rg;
            pd->rg = NULL;
            pd->r_scope = NULL;
            pd->r_deadline = 0;
            w->val = ZAP_POLL_OK;
            coready(w);
        } else {
            pd->r_ready = 1; /* latch: the next read wait returns at once */
        }
    }
    if (mode & ZAP_POLL_WRITE) {
        if (pd->wg) {
            Waiter *w = pd->wg;
            pd->wg = NULL;
            pd->w_scope = NULL;
            pd->w_deadline = 0;
            w->val = ZAP_POLL_OK;
            coready(w);
        } else {
            pd->w_ready = 1; /* latch: the next write wait returns at once */
        }
    }
    zap_np_unlock(&pd->mu);
}

/* Deregister `fd` and release the descriptor. Any waiter still parked is woken
 * with ZAP_POLL_CLOSED. The caller still owns (and should close) the fd itself. */
void zap_netpoll_close(void *pd_) {
    PollDesc *pd = (PollDesc *)pd_;
    zap_np_lock(&g_np_mu);
    zap_np_lock(&pd->mu);
    pd->closing = 1;
    np_backend_del(pd);
    if (pd->rg) {
        Waiter *w = pd->rg;
        pd->rg = NULL;
        pd->r_scope = NULL;
        w->val = ZAP_POLL_CLOSED;
        coready(w);
    }
    if (pd->wg) {
        Waiter *w = pd->wg;
        pd->wg = NULL;
        pd->w_scope = NULL;
        w->val = ZAP_POLL_CLOSED;
        coready(w);
    }
    np_all_remove_locked(pd);
    zap_np_unlock(&pd->mu);
    pd->free_link = g_np_free; /* recycle; a woken waiter never touches pd again */
    g_np_free = pd;
    zap_np_unlock(&g_np_mu);
}

/* Cancel only the waiters owned by this scope tree. Registry -> descriptor
 * order matches close and deadline scanning; no channel lock is held here. */
static void scope_cancel_io(Scope *sc) {
    zap_np_lock(&g_np_mu);
    for (PollDesc *pd = g_np_all; pd; pd = pd->all_next) {
        zap_np_lock(&pd->mu);
        if (pd->rg && pd->r_scope && scope_contains(sc, pd->r_scope)) {
            Waiter *w = pd->rg;
            pd->rg = NULL; pd->r_scope = NULL; pd->r_deadline = 0;
            w->val = ZAP_POLL_CANCELLED;
            coready(w);
        }
        if (pd->wg && pd->w_scope && scope_contains(sc, pd->w_scope)) {
            Waiter *w = pd->wg;
            pd->wg = NULL; pd->w_scope = NULL; pd->w_deadline = 0;
            w->val = ZAP_POLL_CANCELLED;
            coready(w);
        }
        zap_np_unlock(&pd->mu);
    }
    zap_np_unlock(&g_np_mu);
}

#ifdef ZAP_GC_IMMIX
/* Borrowed scope addresses in malloc PollDescs must be pinned, not merely kept
 * alive through the owning G's relocating slot. */
static void zap_netpoll_mark_roots(void) {
    zap_np_lock(&g_np_mu);
    for (PollDesc *pd = g_np_all; pd; pd = pd->all_next) {
        zap_np_lock(&pd->mu);
        if (pd->r_scope) zap_gc_mark(pd->r_scope);
        if (pd->w_scope) zap_gc_mark(pd->w_scope);
        zap_np_unlock(&pd->mu);
    }
    zap_np_unlock(&g_np_mu);
}
#endif

/* Saturating conversion used by all relative read budgets, not just new APIs. */
static int64_t zap_deadline_after_ms(int64_t ms) {
    if (ms <= 0) return 0;
    int64_t now = np_now_ns();
    return ms > (INT64_MAX - now) / 1000000LL ? INT64_MAX : now + ms * 1000000LL;
}

/* Cancellation/deadline check for nonblocking fast paths. Does not raise: the
 * owning native operation must first free its scratch/partial connection. */
static int zap_io_status(void *pd_, int64_t deadline_ns) {
    if (g_is_cancelled()) return -3;
    if (pd_) {
        PollDesc *pd = (PollDesc *)pd_;
        zap_np_lock(&pd->mu);
        int64_t configured = pd->io_deadline_ns;
        zap_np_unlock(&pd->mu);
        if (configured && (!deadline_ns || configured < deadline_ns)) deadline_ns = configured;
    }
    return deadline_ns && np_now_ns() >= deadline_ns ? -2 : 0;
}

/* ---- integration self-test (ticket #80) ----------------------------------- *
 * The netpoller has no language-level I/O surface yet (sockets/http are #79,
 * signals #77), so this drives it end-to-end from C and backs the
 * `__netpoll_selftest()` golden (run under both collectors). It forces a SINGLE
 * worker M (`zap_sched_init(1)`) so the "a parked G frees its M" property is
 * load-bearing: a reader coroutine blocks on a not-yet-ready pipe and MUST park
 * (freeing the one M) or the writer coroutine could never be scheduled and the
 * program would deadlock. Returns the number of sub-checks that passed
 * (ZAP_NP_SELFTEST_CHECKS on full success); prints to stderr on any failure. */
#define ZAP_NP_SELFTEST_CHECKS 2

#if defined(ZAP_NETPOLL_EPOLL)
static int g_np_t_rfd;        /* read end polled by the reader coroutine        */
static int g_np_t_wfd;        /* write end the writer coroutine writes to       */
static void *g_np_t_pd;       /* its PollDesc                                   */
static void *g_np_t_done;     /* buffered channel the two coroutines signal on  */
static volatile int g_np_t_byte;   /* byte the reader received                  */
static volatile int g_np_t_status; /* reader's zap_netpoll_wait outcome         */

static void np_set_nonblock(int fd) {
    int fl = fcntl(fd, F_GETFL, 0);
    if (fl >= 0) {
        fcntl(fd, F_SETFL, fl | O_NONBLOCK);
    }
}

/* Reader coroutine: parks on the pipe until readable, then drains one byte. */
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

/* Writer coroutine: writes the sentinel byte to wake the parked reader. Runs on
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
            /* Native wait for both coroutines (parks main on the channel). */
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
 * "dispatcher coroutine". Here the dedicated signal thread is itself the reader/
 * dispatcher — it still parks on the netpoller-registered self-pipe and delivers
 * in normal context, and fan-out is a non-blocking, non-allocating channel offer
 * that is scheduler-safe from a native (non-M) thread. A distinct dispatcher
 * coroutine would force the M:N worker pool onto EVERY binary (default handlers
 * are on by default), breaking the deliberate scheduler-laziness invariant that
 * keeps non-concurrent programs zero-cost (#47). `@on_signal` delivery DOES run
 * on a real coroutine (a detached receiver loop over the notify channel).
 *
 * FAULT signals (SIGSEGV/SIGFPE/SIGBUS/SIGILL) are NOT delivered via `notify`: a
 * crash handler (on a sigaltstack, so a stack overflow is still reportable)
 * prints the faulting signal/address, the offending coroutine, and a native
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
    int64_t timeout_ms;
    int completed; /* scope left its block (disarmed) before any hard-exit       */
    int refcnt;    /* 1 (list) + 1 per outstanding grace timer                   */
} ScopeSigSub;

static pthread_mutex_t g_sig_mu = PTHREAD_MUTEX_INITIALIZER;
static SigSub *g_sig_subs[ZAP_NSIG];        /* notify() subscribers, per signo   */
static ScopeSigSub *g_scope_deadlines;
static ScopeSigSub *g_scope_subs[ZAP_NSIG]; /* cancel_on subscriptions, per signo */
static int g_sig_default[ZAP_NSIG];         /* 1 => run the built-in default      */
static int g_sig_installed[ZAP_NSIG];       /* OS handler installed for this sig  */

static int g_sig_pipe[2] = {-1, -1}; /* [0]=read (netpoller), [1]=write (handler)*/
static void *g_sig_pd = NULL;        /* PollDesc for the read end                */
static pthread_t g_sig_thread;
static volatile int g_sig_active = 0;    /* subsystem started                    */
static int g_sig_shutdown = 0;  /* signal thread asked to stop (#213, D-0121):
                                 * exit latch read by the signal thread after each
                                 * netpoller wait and set by the shutdown path,
                                 * which then wakes + joins it. RELAXED __atomic,
                                 * not plain volatile — no payload; the wake plus
                                 * re-poll give liveness.                         */
static pthread_once_t g_sig_once = PTHREAD_ONCE_INIT;
/* Async-handler run count, polled by zap_raise_signal for FIFO test sync (#213,
 * D-0121). Bumped inside a real async signal handler and read on another thread,
 * so it is accessed via RELAXED `__atomic_*` — a monotone counter with no
 * payload; `__atomic` on an aligned sig_atomic_t stays async-signal-safe and
 * lock-free. Not plain `volatile`, which would give the cross-thread poll no
 * well-defined load on weak-memory arches. */
static sig_atomic_t g_sig_delivered = 0;

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
    zap_np_lock(&c->mu);
    if (c->closed) {
        zap_np_unlock(&c->mu);
        return 0;
    }
    if (c->count == 0 && c->recvq_head) {
        Waiter *r = q_pop(&c->recvq_head, &c->recvq_tail);
        r->val = v;
        r->ok = 1;
        coready(r);
        zap_np_unlock(&c->mu);
        return 1;
    }
    if (c->cap > 0 && c->count < c->cap) {
        c->buf[c->tail] = v;
        c->tail = (c->tail + 1) % c->cap;
        c->count++;
        zap_np_unlock(&c->mu);
        return 1;
    }
    zap_np_unlock(&c->mu);
    return 0; /* coalesce: drop this edge */
}

/* ---- diagnostics: coroutine / scheduler dump (SIGQUIT default, runtime.*) -- */
static const char *gstate_name(GState s) {
    switch (s) {
    case G_RUNNABLE: return "runnable";
    case G_RUNNING: return "running";
    case G_PARKED: return "parked";
    case G_DEAD: return "dead";
    default: return "?";
    }
}

/* Count live coroutines (runnable+running+parked). Reads the static g_all[]
 * registry under its lock; safe from any thread. */
int64_t zap_runtime_num_coroutines(void) {
    zap_np_lock(&g_all_mu);
    int64_t n = g_all_len;
    zap_np_unlock(&g_all_mu);
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

/* Dump every live coroutine + a scheduler summary to `f`. v1 reports per-G
 * identity/state/ownership and the scheduler/GC shape; full per-G symbolicated
 * stack traces (unwinding each green stack) are a follow-up (noted in the PR). */
static void zap_coroutine_dump_to(FILE *f) {
    int64_t ng, maxprocs, live;
    zap_np_lock(&g_live_mu);
    live = g_live;
    zap_np_unlock(&g_live_mu);
    maxprocs = (int64_t)g_nm;
    fprintf(f, "=== zap runtime dump ===\n");
    fprintf(f, "MAXPROCS(M=P): %lld  root-live coroutines: %lld\n",
            (long long)maxprocs, (long long)live);
    int64_t heap = 0, lb = 0;
    zap_gc_stats(&heap, &lb);
    fprintf(f, "GC: cycles=%lld heap=%lld bytes live=%lld bytes\n",
            (long long)zap_gc_count(), (long long)heap, (long long)lb);
    zap_np_lock(&g_all_mu);
    ng = g_all_len;
    fprintf(f, "coroutines: %lld\n", (long long)ng);
    for (int64_t i = 0; i < g_all_len; i++) {
        G *g = g_all[i];
        if (!g) {
            continue;
        }
        const char *own = g->severed ? "severed (leaked)"
                          : g->scope  ? "scoped"
                          : g->in_live ? "root"
                                       : "detached";
        const char *nm = g->name ? g->name : "<anonymous>";
        if (g->state == G_PARKED && g->park_reason) {
            fprintf(f, "  coroutine %lld %s [%s on %s, %s]\n", (long long)i, nm,
                    gstate_name(g->state), g->park_reason, own);
        } else {
            fprintf(f, "  coroutine %lld %s [%s, %s]\n", (long long)i, nm,
                    gstate_name(g->state), own);
        }
    }
    zap_np_unlock(&g_all_mu);
    fprintf(f, "========================\n");
    fflush(f);
}

void zap_runtime_coroutine_dump(void) { zap_coroutine_dump_to(stderr); }

/* ---- default diagnostic handlers ------------------------------------------ */
static void zap_sig_run_default(int signo) {
    if (signo == SIGUSR1) {
        zap_gc_collect();
        int64_t heap = 0, live = 0;
        zap_gc_stats(&heap, &live);
        fprintf(stderr,
                "=== zap SIGUSR1: forced GC ===\n"
                "heap=%lld bytes  live=%lld bytes  gc-cycles=%lld  coroutines=%lld\n",
                (long long)heap, (long long)live, (long long)zap_gc_count(),
                (long long)zap_runtime_num_coroutines());
        fflush(stderr);
    } else if (signo == SIGQUIT) {
        zap_coroutine_dump_to(stderr);
    } else if (signo == SIGUSR2) {
        /* D-0058: SIGUSR2 toggles the CPU profiler. Runs on the dedicated signal
         * thread in normal context, so start/stop may allocate, open the output
         * file, start the monitor thread, and symbolicate at stop. */
        zap_profile_toggle();
    }
}

/* ---- dispatch (runs on the dedicated signal thread, normal context) ------- */
static void zap_grace_arm(ScopeSigSub *ss); /* fwd */
static void zap_sig_dispatch(int signo) {
    if (signo <= 0 || signo >= ZAP_NSIG) {
        return;
    }
    zap_np_lock(&g_sig_mu);
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
        scope_request_cancel(sc);
        if (ss->grace_ms > 0) {
            zap_grace_arm(ss);
        }
    }
    int run_default = (!had_sub) && g_sig_default[signo];
    zap_np_unlock(&g_sig_mu);
    /* scope_request_cancel also dequeues and wakes cancelled channel waiters. */
    if (run_default) {
        zap_sig_run_default(signo);
    }
}

/* ---- the dedicated signal thread (POSIX) ---------------------------------- *
 * The self-pipe + netpoller + reader-thread transport is a POSIX-signal
 * mechanism. On Windows there are no async POSIX signals: the A3 (#197) console
 * control handler calls `zap_sig_dispatch` directly from its own OS thread (no
 * self-pipe, no netpoller — design/0019 §5 decouples the two), so this whole
 * transport is POSIX-only and the Windows start/shutdown paths below install the
 * console handler + fault filter instead. */
#if defined(ZAP_PLATFORM_POSIX)
/* async-signal-safe: only writes the signo byte to the self-pipe (write(2) is
 * async-signal-safe; a full non-blocking pipe simply coalesces the edge). */
static void zap_sig_handler(int signo) {
    int saved = errno;
    unsigned char b = (unsigned char)signo;
    ssize_t r = write(g_sig_pipe[1], &b, 1);
    (void)r;
    /* async-signal-safe relaxed atomic bump (sig_atomic_t): test sync counter */
    __atomic_fetch_add(&g_sig_delivered, 1, __ATOMIC_RELAXED);
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
     * copark's native path, which brackets the wait with enter/leave_blocking;
     * it never allocates GC memory and is GC-safe while blocked (like main). */
    zap_mutator_register();
    unsigned char buf[64];
    for (;;) {
        int64_t r = zap_netpoll_wait(g_sig_pd, ZAP_POLL_READ, 0);
        if (__atomic_load_n(&g_sig_shutdown, __ATOMIC_RELAXED)) {
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
                    zap_np_lock(&g_sig_sync_mu);
                    g_sig_sync_seq++;
                    pthread_cond_broadcast(&g_sig_sync_cv);
                    zap_np_unlock(&g_sig_sync_mu);
                } else {
                    zap_sig_dispatch((int)buf[i]);
                }
            }
        }
    }
    return NULL;
}
#endif /* ZAP_PLATFORM_POSIX (self-pipe signal transport) */

/* ---- fault (crash) handler ------------------------------------------------ */
#if defined(ZAP_PLATFORM_POSIX)
static void zap_fault_handler(int signo, siginfo_t *info, void *uctx) {
    (void)uctx;
    char msg[256];
    void *addr = info ? info->si_addr : NULL;
    M *m = cur_m;
    G *g = m ? m->current : NULL;
    int n = snprintf(msg, sizeof msg,
                     "\n=== zap fatal: %s (signal %d) ===\n"
                     "faulting address: %p\n"
                     "offending coroutine: ",
                     sig_name(signo), signo, addr);
    if (n > 0) {
        ssize_t w = write(STDERR_FILENO, msg, (size_t)n);
        (void)w;
    }
    if (g) {
        int m2 = snprintf(msg, sizeof msg, "coroutine %lld [%s]\n",
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
#else  /* ZAP_PLATFORM_WINDOWS: faults via the A3 last-chance filter */
/* Mirror the POSIX fault handler's middle line ("offending coroutine: ...") into
 * `buf`; zap_win_fault_filter (zap_win_a3.c) prints the type/address header and
 * the dbghelp backtrace around it. A hardware fault (SEH exception) is reported
 * by SetUnhandledExceptionFilter, which then lets the OS default run — the
 * analogue of the POSIX handler's "restore SIG_DFL and re-raise". */
static void zap_win_fault_describe_cb(char *buf, size_t n) {
    M *m = cur_m;
    G *g = m ? m->current : NULL;
    if (g) {
        snprintf(buf, n, "coroutine %lld [%s]\n", (long long)g->all_idx,
                 gstate_name(g->state));
    } else {
        snprintf(buf, n, "main/native thread\n");
    }
}
static void zap_install_fault_handlers(void) {
    zap_win_fault_describe = zap_win_fault_describe_cb;
    zap_win_install_fault_filter();
}
#endif /* ZAP_PLATFORM_POSIX fault handler */

/* Install (idempotently) the handler for `signo`. Caller holds g_sig_mu. POSIX:
 * the async self-pipe handler, SA_RESTART so a slow syscall resumes; also block
 * the signal on the installing thread so it is delivered to the dedicated signal
 * thread. Windows: the single SetConsoleCtrlHandler (installed in start_once)
 * already catches every console-reachable signal, so this just records the
 * subscription — a non-console signal is inert but still table-tracked. */
static void zap_sig_install(int signo) {
    if (signo <= 0 || signo >= ZAP_NSIG || g_sig_installed[signo]) {
        return;
    }
#if defined(ZAP_PLATFORM_WINDOWS)
    g_sig_installed[signo] = 1;
#else
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
#endif
}

/* ---- lazy start / shutdown ------------------------------------------------ */
#if defined(ZAP_PLATFORM_WINDOWS)
/* The console control handler runs on an OS-spawned thread (A3/#197): route a
 * mapped console event straight into the shared dispatch core. Returns 1 iff a
 * user subscription (notify / cancel_on) exists for the signal, so A3 suppresses
 * the OS default (process termination); otherwise the default (terminate) runs. */
static int zap_win_console_deliver_cb(int signo) {
    if (signo <= 0 || signo >= ZAP_NSIG) {
        return 0;
    }
    zap_np_lock(&g_sig_mu);
    int had_sub = (g_sig_subs[signo] != NULL) || (g_scope_subs[signo] != NULL);
    zap_np_unlock(&g_sig_mu);
    zap_sig_dispatch(signo);
    return had_sub;
}

static void zap_sig_start_once(void) {
    /* No self-pipe/netpoller/reader-thread on Windows: install the last-chance
     * fault filter and the console control handler, wiring the latter to the
     * dispatch core. Signal delivery is synchronous on the handler's own thread. */
    zap_install_fault_handlers();
    zap_win_console_deliver = zap_win_console_deliver_cb;
    if (zap_win_console_install() != 0) {
        fprintf(stderr, "zap: SetConsoleCtrlHandler failed\n");
        return;
    }
    g_sig_active = 1;
}
#else
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
    int rc = zap_gc_pthread_create(&g_sig_thread, NULL, zap_sig_thread_main, NULL);
    if (rc != 0) {
        fprintf(stderr, "zap: signal thread start failed (%d)\n", rc);
        return;
    }
    g_sig_active = 1;
}
#endif /* ZAP_PLATFORM_WINDOWS signal start */

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
#if defined(ZAP_PLATFORM_WINDOWS)
    /* No reader thread or self-pipe to tear down on Windows: just remove the
     * console control handler. Dispatch is synchronous on the handler's thread. */
    __atomic_store_n(&g_sig_shutdown, 1, __ATOMIC_RELAXED);
    zap_win_console_uninstall();
    zap_win_console_deliver = NULL;
    g_sig_active = 0;
    return;
#else
    __atomic_store_n(&g_sig_shutdown, 1, __ATOMIC_RELAXED);
    /* Wake the signal thread off its netpoller park so it observes g_sig_shutdown
     * and exits. zap_netpoll_wake now LATCHES when the thread is momentarily
     * between waits (not parked), so this wake is race-free — a plain self-pipe
     * write would rely on an EPOLLET edge that is lost if the pipe is non-empty,
     * which is exactly how this join used to hang intermittently. */
    if (g_sig_pd) {
        zap_netpoll_wake(g_sig_pd, ZAP_POLL_READ);
    }
    zap_gc_pthread_join(g_sig_thread, NULL);
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
#endif /* ZAP_PLATFORM_WINDOWS signal shutdown */
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
    zap_np_lock(&g_sig_mu);
    g_sig_default[SIGQUIT] = 1;
    zap_sig_install(SIGQUIT);
    g_sig_default[SIGUSR1] = 1;
    zap_sig_install(SIGUSR1);
    /* SIGUSR2 -> toggle the CPU profiler (ticket #78, D-0058). Like the SIGQUIT/
     * SIGUSR1 defaults, a user `notify`/`@on_signal` for SIGUSR2 overrides it. */
    g_sig_default[SIGUSR2] = 1;
    zap_sig_install(SIGUSR2);
    zap_np_unlock(&g_sig_mu);
    /* Parse ZAP_PROFILE / ZAP_PROFILE_HZ / ZAP_PROFILE_OUT now (main top); the
     * auto-start itself is deferred to zap_sched_init, once the worker Ms that a
     * CPU profile samples actually exist. */
    zap_profile_env_init();
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
    zap_np_lock(&g_sig_mu);
    s->next = g_sig_subs[signo];
    g_sig_subs[signo] = s;
    zap_sig_install((int)signo); /* notify takes over disposition */
    zap_np_unlock(&g_sig_mu);
    return ch;
}

/* ---- public ABI: @on_signal(signo) handler registration ------------------- */
typedef void (*zap_sig_cb)(void);
typedef struct {
    void *ch;
    zap_sig_cb fn;
} SigCbArg;

/* A detached coroutine that invokes the decorated handler on each delivery. It
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
/* Grace HARD-EXIT timer (#825: SIGNAL-driven shutdown ONLY). Reserved for
 * `scope(cancel_on=[SIG...], grace=G)` via `zap_sig_dispatch`, where the process
 * is terminating anyway in response to a shutdown signal — forcing exit is the
 * legitimate backstop there. A per-scope `scope(timeout=...)` deadline must NOT
 * reach this path: it severs the non-draining child and keeps the process alive
 * (`zap_scope_sever_timer` below), so one faulty endpoint never takes the server
 * down. A plain (non-GC) detached thread referencing only the malloc'd,
 * refcounted ScopeSigSub (never the GC Scope), so it is immune to the Scope being
 * collected after the block exits. If the scope has not left its block
 * (completed) when the grace period elapses, force process exit. */
static void *zap_grace_timer(void *arg) {
    ScopeSigSub *ss = (ScopeSigSub *)arg;
    int64_t ms = ss->grace_ms;
    struct timespec ts;
    ts.tv_sec = ms / 1000;
    ts.tv_nsec = (ms % 1000) * 1000000L;
    nanosleep(&ts, NULL);
    zap_np_lock(&g_sig_mu);
    int done = ss->completed;
    if (--ss->refcnt == 0) {
        free(ss);
    }
    zap_np_unlock(&g_sig_mu);
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
    if (zap_gc_pthread_create(&t, NULL, zap_grace_timer, ss) != 0) {
        ss->refcnt--; /* could not arm: drop the extra ref */
        return;
    }
    zap_gc_pthread_detach(t);
}

/* #825: SEVER every coroutine still live in `sc`'s tree that did not drain within
 * the deadline's (grace) window. Severing detaches the coroutine to Go `spawn
 * detached` accounting — it is CLAIMED away from its owning scope (so that scope's
 * join can complete and raise `ScopeTimeout`), marked a leaked orphan
 * (`g->severed`), and left to finish on its own. It is NEVER force-unwound or
 * freed (unsafe mid-FFI / mid-lock — the reason Java deprecated Thread.stop) and
 * NEVER `_exit`s the process. `g->scope` is claimed with an atomic CAS (owner ->
 * NULL): the coroutine's own death path claims the same way, so whichever runs
 * first owns the single owned-count decrement and the other is skipped. Each
 * severed orphan is surfaced to stderr as a leak diagnostic (observability). */
static void scope_sever_live(Scope *sc, int64_t deadline_ms) {
    struct sev_row {
        Scope *owner;
        const char *name;
        const char *park;
    };
    struct sev_row *rows = NULL;
    size_t nrows = 0, cap = 0;
    zap_np_lock(&g_all_mu);
    for (int64_t i = 0; i < g_all_len; i++) {
        G *g = g_all[i];
        if (!g || g->state == G_DEAD) {
            continue;
        }
        Scope *owner = __atomic_load_n(&g->scope, __ATOMIC_ACQUIRE);
        if (!owner || !scope_contains(sc, owner)) {
            continue;
        }
        /* Mark the orphan before clearing its scope, so a severed coroutine that
         * later reaches `zap_g_uncaught` (with scope already NULL) is recognized
         * and its exception dropped rather than aborting the process. */
        g->severed = 1;
        Scope *expected = owner;
        if (!__atomic_compare_exchange_n(&g->scope, &expected, (Scope *)NULL,
                                         /*weak=*/0, __ATOMIC_ACQ_REL,
                                         __ATOMIC_ACQUIRE)) {
            /* The coroutine finished first and its death path already claimed the
             * scope (and decremented the owned count); leave it to that path. */
            continue;
        }
        if (nrows == cap) {
            cap = cap ? cap * 2 : 8;
            rows = (struct sev_row *)realloc(rows, cap * sizeof(*rows));
            if (!rows) {
                abort();
            }
        }
        rows[nrows].owner = owner;
        rows[nrows].name = g->name ? g->name : "<anonymous>";
        rows[nrows].park = (g->state == G_PARKED && g->park_reason)
                               ? g->park_reason
                               : gstate_name(g->state);
        nrows++;
    }
    zap_np_unlock(&g_all_mu);
    /* Release each severed child's owned-count reference and surface the leak.
     * Done outside g_all_mu so waking a parked joiner cannot invert a lock
     * order; the deadline-thread lock order (g_sig_mu -> ... -> sc->mu) holds. */
    for (size_t i = 0; i < nrows; i++) {
        fprintf(stderr,
                "zap: scope timed out after %lld ms; severed non-draining "
                "coroutine '%s' [%s] (leaked; process continues)\n",
                (long long)deadline_ms, rows[i].name, rows[i].park);
        scope_gdone(rows[i].owner);
    }
    if (nrows > 0) {
        fflush(stderr);
    }
    free(rows);
}

/* #825: the per-scope `scope(timeout=T[, grace=G])` drain-then-sever backstop —
 * the TIMEOUT counterpart of the signal `zap_grace_timer`, but it NEVER `_exit`s.
 * After the grace window (G ms; G == 0 for a bare `scope(timeout=T)` => the
 * maintainer-ruled implicit grace=0, i.e. sever immediately), if the scope has not
 * left its block, sever whatever is still running so the join completes and
 * `ScopeTimeout` reaches the caller. Like the grace timer it is a plain detached
 * thread holding only the refcounted ScopeSigSub; it touches the GC Scope only
 * while `completed` is still 0 (checked under g_sig_mu), during which the scope's
 * own frame keeps it rooted. Holding g_sig_mu across the sever keeps the block
 * from disarming (and the Scope from being collected) mid-walk. */
static void *zap_scope_sever_timer(void *arg) {
    ScopeSigSub *ss = (ScopeSigSub *)arg;
    /* Register as a GC mutator: `scope_sever_live` reads and rewrites fields of
     * live GC objects (the Scope and each owned G), so under Immix evacuation the
     * collector must not move them out from under this thread. Same discipline as
     * `zap_scope_deadline_main`. The sever itself performs no GC allocation. */
    zap_mutator_register();
    int64_t ms = ss->grace_ms;
    if (ms > 0) {
        struct timespec ts;
        ts.tv_sec = ms / 1000;
        ts.tv_nsec = (ms % 1000) * 1000000L;
        struct timespec rem;
        zap_enter_blocking(); /* GC-safe while sleeping out the grace window */
        while (nanosleep(&ts, &rem) != 0 && errno == EINTR) {
            ts = rem;
        }
        zap_leave_blocking();
    }
    zap_np_lock(&g_sig_mu);
    if (!ss->completed) {
        scope_sever_live(ss->scope, ss->timeout_ms);
    }
    if (--ss->refcnt == 0) {
        free(ss);
    }
    zap_np_unlock(&g_sig_mu);
    zap_mutator_unregister();
    return NULL;
}

/* Caller holds g_sig_mu. Arm the timeout sever backstop for this deadline. */
static void zap_scope_sever_arm(ScopeSigSub *ss) {
    ss->refcnt++;
    pthread_t t;
    if (zap_gc_pthread_create(&t, NULL, zap_scope_sever_timer, ss) != 0) {
        ss->refcnt--; /* could not arm: drop the extra ref */
        return;
    }
    zap_gc_pthread_detach(t);
}

/* Build the `ScopeTimeout` the timed-out scope raises to its caller (I#475). The
 * message carries the deadline and the COMPLETE set of coroutines still
 * outstanding at the deadline, captured by `scope_fire_timeout` before any were
 * cancelled — so a caught timeout never implies the unreported ones succeeded.
 * The rendered buffer is malloc'd (not GC-managed); copy it into a GC string and
 * release it here, since the exception is built exactly once per timed-out scope. */
static void *scope_timeout_exc(Scope *sc) {
    /* Detach the rendered buffer under the lock, then build the GC string outside
     * it — no GC allocation is performed while holding `sc->mu`. */
    zap_np_lock(&sc->mu);
    char *buf = sc->timeout_msg;
    int64_t buflen = sc->timeout_msg_len;
    int64_t deadline = sc->deadline_ms;
    sc->timeout_msg = NULL;
    sc->timeout_msg_len = 0;
    zap_np_unlock(&sc->mu);
    void *msg;
    if (buf) {
        msg = zap_str_new(buf, buflen);
        free(buf);
    } else {
        char tmp[64];
        int n = snprintf(tmp, sizeof(tmp), "scope timed out after %lld ms",
                         (long long)deadline);
        msg = zap_str_new(tmp, n < 0 ? 0 : n);
    }
    return zap_exception_new(ZAP_EXC_SCOPE_TIMEOUT, msg);
}

/* A deadline (or an immediate `timeout=0`) fired on `sc`. Snapshot every
 * coroutine still outstanding in `sc`'s tree — the complete set by construction,
 * since the snapshot runs BEFORE cancellation drains them — render the report,
 * record it on the scope, then cancel the tree through the shared channel-aware
 * path. The scope's exit reads the report back to build `ScopeTimeout`. Only the
 * name + last park reason are reported; both are static C-strings already carried
 * on the G, so no allocation happens under `g_all_mu`. */
static void scope_fire_timeout(Scope *sc, int64_t deadline_ms) {
    struct pend_row { const char *name; const char *park; } *rows = NULL;
    size_t nrows = 0, cap = 0;
    zap_np_lock(&g_all_mu);
    for (int64_t i = 0; i < g_all_len; i++) {
        G *g = g_all[i];
        if (!g || g->state == G_DEAD || !scope_contains(sc, g->scope)) {
            continue;
        }
        if (nrows == cap) {
            cap = cap ? cap * 2 : 8;
            rows = (struct pend_row *)realloc(rows, cap * sizeof(*rows));
            if (!rows) abort();
        }
        rows[nrows].name = g->name ? g->name : "<anonymous>";
        rows[nrows].park = (g->state == G_PARKED && g->park_reason)
                               ? g->park_reason
                               : gstate_name(g->state);
        nrows++;
    }
    zap_np_unlock(&g_all_mu);

    size_t len = 0, mcap = 256;
    char *msg = (char *)malloc(mcap);
    if (!msg) abort();
#define SCOPE_TIMEOUT_APPEND(...)                                              \
    do {                                                                       \
        for (;;) {                                                             \
            int need = snprintf(msg + len, mcap - len, __VA_ARGS__);           \
            if (need < 0) abort();                                             \
            if ((size_t)need < mcap - len) {                                   \
                len += (size_t)need;                                           \
                break;                                                         \
            }                                                                  \
            mcap = (len + (size_t)need + 1) * 2;                               \
            msg = (char *)realloc(msg, mcap);                                  \
            if (!msg) abort();                                                 \
        }                                                                      \
    } while (0)
    SCOPE_TIMEOUT_APPEND("scope timed out after %lld ms with %zu coroutine(s) still running",
                         (long long)deadline_ms, nrows);
    if (nrows == 0) {
        SCOPE_TIMEOUT_APPEND(".");
    } else {
        SCOPE_TIMEOUT_APPEND(":");
        for (size_t i = 0; i < nrows; i++) {
            SCOPE_TIMEOUT_APPEND("\n  - %s [parked on %s]", rows[i].name, rows[i].park);
        }
    }
#undef SCOPE_TIMEOUT_APPEND
    free(rows);

    zap_np_lock(&sc->mu);
    sc->timed_out = 1;
    sc->deadline_ms = deadline_ms;
    if (sc->timeout_msg) {
        free(sc->timeout_msg); /* defensive: a scope never fires twice (disarm) */
    }
    sc->timeout_msg = msg;
    sc->timeout_msg_len = (int64_t)len;
    zap_np_unlock(&sc->mu);
    scope_request_cancel(sc);
}

static void *zap_scope_deadline_main(void *arg) {
    ScopeSigSub *ss = (ScopeSigSub *)arg;
    zap_mutator_register();
    struct timespec remaining = { ss->timeout_ms / 1000, (ss->timeout_ms % 1000) * 1000000L };
    zap_enter_blocking();
    while (nanosleep(&remaining, &remaining) != 0 && errno == EINTR) {}
    zap_leave_blocking();
    zap_np_lock(&g_sig_mu);
    if (!ss->completed) {
        scope_fire_timeout(ss->scope, ss->timeout_ms);
        /* #825: a per-scope timeout NEVER hard-exits. Arm the drain-then-sever
         * backstop (grace=0 => sever immediately) instead of the signal-shutdown
         * `_exit` grace timer, so a non-draining child is severed-and-leaked and
         * the scope raises `ScopeTimeout` to its caller with the process alive. */
        zap_scope_sever_arm(ss);
    }
    if (--ss->refcnt == 0) free(ss);
    zap_np_unlock(&g_sig_mu);
    zap_mutator_unregister();
    return NULL;
}

/* `scope(timeout=<ms>[, grace=<ms>])` (I#475, D-0094): arm a relative deadline in
 * integer milliseconds. `timeout=0` cancels immediately; a negative duration is a
 * `ValueError`. At expiry the tree is cancelled (running `finally`/`with`) and the
 * scope then raises `ScopeTimeout` to its caller. Orthogonal to `on_error` and
 * `cancel_on`. A child that does not drain within `grace` ms (grace=0 for a bare
 * `scope(timeout=T)`: the implicit-grace=0 ruling, #825) is SEVERED-and-leaked,
 * never force-exited and never hung — `grace` only widens the cooperative drain
 * window before the sever. Hard process exit (`_exit`) is reserved for
 * signal-driven shutdown (`scope(cancel_on=...)`). */
void zap_scope_timeout(void *scope_, int64_t duration_ms, int64_t grace_ms) {
    if (duration_ms < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("scope timeout must be a nonnegative number of milliseconds", 58));
    }
    ScopeSigSub *ss = (ScopeSigSub *)calloc(1, sizeof(*ss));
    if (!ss) abort();
    ss->scope = (Scope *)scope_; ss->timeout_ms = duration_ms;
    ss->grace_ms = grace_ms; ss->refcnt = 2;
    zap_np_lock(&g_sig_mu);
    ss->next = g_scope_deadlines; g_scope_deadlines = ss;
    if (ss->timeout_ms == 0) {
        scope_fire_timeout(ss->scope, 0);
        zap_scope_sever_arm(ss); /* #825: sever-and-leak, never `_exit` */
        ss->refcnt--; /* no deadline thread for immediate cancellation */
        zap_np_unlock(&g_sig_mu);
        return;
    }
    pthread_t timer;
    if (zap_gc_pthread_create(&timer, NULL, zap_scope_deadline_main, ss) != 0) {
        g_scope_deadlines = ss->next; free(ss);
        zap_np_unlock(&g_sig_mu);
        zap_panic_typed(ZAP_EXC_RUNTIME_ERROR, zap_str_new("cannot create scope deadline timer", 34));
        return;
    }
    zap_gc_pthread_detach(timer);
    zap_np_unlock(&g_sig_mu);
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
    zap_np_lock(&g_sig_mu);
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
    zap_np_unlock(&g_sig_mu);
}

/* Remove every cancel_on subscription for `scope` (its block has exited). Marks
 * each node completed so a pending grace timer stands down, and frees nodes with
 * no outstanding timer. */
void zap_scope_disarm(void *scope_) {
    Scope *sc = (Scope *)scope_;
    zap_np_lock(&g_sig_mu);
    ScopeSigSub **deadline = &g_scope_deadlines;
    while (*deadline) {
        ScopeSigSub *n = *deadline;
        if (n->scope == sc) {
            *deadline = n->next; n->completed = 1;
            if (--n->refcnt == 0) free(n);
        } else { deadline = &n->next; }
    }
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
    zap_np_unlock(&g_sig_mu);
}

/* ---- Immix root marker: keep notify channels + sub nodes reachable -------- */
#ifdef ZAP_GC_IMMIX
static void zap_sig_mark_roots(void) {
    zap_np_lock(&g_sig_mu);
    for (ScopeSigSub *n = g_scope_deadlines; n; n = n->next) zap_gc_mark(n->scope);
    for (int s = 0; s < ZAP_NSIG; s++) {
        /* Marking each head traces the whole {next, chan} chain via the precise
         * SigSub descriptor (the marker is a transitive BFS worklist). */
        zap_gc_mark(g_sig_subs[s]);
    }
    zap_np_unlock(&g_sig_mu);
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
#if defined(ZAP_PLATFORM_WINDOWS)
    /* No kill(2)/process-directed signals on Windows. A fault signal terminates
     * via the CRT raise(); any other signal is delivered straight through the
     * (synchronous) dispatch core — the same path the console handler uses. */
    if (is_fault) {
        raise(s);
        return;
    }
    if (g_sig_active) {
        zap_sig_dispatch(s);
    }
#else
    if (is_fault || !g_sig_active) {
        kill(getpid(), s); /* fault: the crash handler runs + re-raises (no return) */
        return;
    }
    sig_atomic_t before = __atomic_load_n(&g_sig_delivered, __ATOMIC_RELAXED);
    kill(getpid(), s);
    /* Spin until the async handler has written this edge to the self-pipe, so a
     * following __signal_drain sees it first in FIFO order. Bounded so a dropped
     * signal cannot hang a test. */
    for (int i = 0;
         i < 2000000 &&
         __atomic_load_n(&g_sig_delivered, __ATOMIC_RELAXED) == before;
         i++) {
        sched_yield();
    }
#endif
}

/* Block until the signal thread has dispatched every edge raised so far
 * (ticket #77 test sync; backs `__signal_drain`). Pushes a sentinel behind the
 * pending edges and waits for the signal thread to reach it. */
void zap_signal_drain(void) {
    if (!g_sig_active) {
        return;
    }
#if defined(ZAP_PLATFORM_WINDOWS)
    /* Windows dispatch is synchronous (the console handler calls zap_sig_dispatch
     * inline, and zap_signal_raise dispatches directly), so every edge raised so
     * far has already been delivered — nothing to drain. */
    return;
#else
    /* Precise GC (#48): this caller (typically a mutator — e.g. the main thread)
     * blocks on the condvar below, so mark it GC-safe. Otherwise a SIGUSR1
     * default handler's forced GC (stop-the-world, run on the signal thread)
     * would deadlock waiting for this thread to reach a safepoint it can never
     * reach while parked here. Its roots were flushed on entry (same contract as
     * copark's native-waiter path / scope_join). */
    zap_enter_blocking();
    zap_np_lock(&g_sig_sync_mu);
    int64_t want = g_sig_sync_seq + 1;
    unsigned char b = ZAP_SIG_SYNC_BYTE;
    ssize_t r;
    do {
        r = write(g_sig_pipe[1], &b, 1);
    } while (r < 0 && errno == EINTR);
    while (g_sig_sync_seq < want) {
        pthread_cond_wait(&g_sig_sync_cv, &g_sig_sync_mu);
    }
    zap_np_unlock(&g_sig_sync_mu);
    zap_leave_blocking();
#endif /* ZAP_PLATFORM_WINDOWS */
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
    /* Deliver two edges; coalescing means the cap-1 channel holds one. */
#if defined(ZAP_PLATFORM_WINDOWS)
    /* Windows dispatch is synchronous (zap_signal_raise -> zap_sig_dispatch), so
     * after the first raise the cap-1 channel already holds the edge and the
     * second coalesces; no spin/settle needed. SIGUSR2 is not console-reachable,
     * which is exactly why the direct-dispatch path is exercised here. */
    zap_signal_raise(SIGUSR2);
    zap_signal_raise(SIGUSR2); /* second edge: buffer full -> coalesced (dropped) */
#else
    /* POSIX: must be PROCESS-directed (kill(getpid)), not thread-directed (raise):
     * the handled signal is masked on this thread and unblocked only on the signal
     * thread. */
    kill(getpid(), SIGUSR2);
    /* Spin briefly so the signal thread delivers the first edge before the
     * second arrives (keeps the test deterministic without a second channel). */
    for (int i = 0; i < 100; i++) {
        struct timespec ts = {0, 1000000L}; /* 1 ms */
        nanosleep(&ts, NULL);
        zap_np_lock(&((ZapChan *)ch)->mu);
        int64_t cnt = ((ZapChan *)ch)->count;
        zap_np_unlock(&((ZapChan *)ch)->mu);
        if (cnt > 0) {
            break;
        }
    }
    kill(getpid(), SIGUSR2); /* second edge: buffer full -> coalesced (dropped) */
    struct timespec ts = {0, 20000000L}; /* 20 ms settle */
    nanosleep(&ts, NULL);
#endif
    int64_t got = zap_chan_recv_i64(ch); /* one delivery */
    if (got == SIGUSR2) {
        passed++;
    } else {
        fprintf(stderr, "signal selftest: delivered %lld, want %d\n",
                (long long)got, SIGUSR2);
    }
    /* The coalesced edge must have left the channel empty (no second value). */
    zap_np_lock(&((ZapChan *)ch)->mu);
    int64_t remaining = ((ZapChan *)ch)->count;
    zap_np_unlock(&((ZapChan *)ch)->mu);
    if (remaining == 0) {
        passed++;
    } else {
        fprintf(stderr, "signal selftest: %lld edges not coalesced\n",
                (long long)remaining);
    }
    return passed;
}

#include "runtime/os_fs.inc.c"
#include "runtime/crypto.inc.c"
/* uuid (#975, D-0232/D-0233): after crypto.inc.c — reuses its CSPRNG/MD5/SHA-1. */
#include "runtime/uuid.inc.c"
#include "runtime/bytes.inc.c"
/* Bundled IANA timezone tables (#1045, D-0225) — must precede time_datetime,
 * which binary-searches ZAP_TZ_ZONES. */
#include "runtime/tzdata_generated.inc.c"
#include "runtime/time_datetime.inc.c"
/* ===========================================================================
 * Type descriptors for the runtime's own heap types (ticket #48). Both GC
 * builds read `.size`; the precise Immix collector also reads `.kind` and the
 * pointer-offset arrays. Codegen emits the descriptors for *user* structs and
 * closures; these cover the built-in runtime objects. See design/0008-precise-gc.
 * ========================================================================== */

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
const ZapTypeDesc zap_desc_atomicarray =
    {(int64_t)sizeof(ZapAtomicArray), ZAP_TD_ATOMICARRAY, 0, NULL};
/* ZapPy wraps a borrowed CPython reference, not a GC pointer, so it is a leaf. */
const ZapTypeDesc zap_desc_pyobj = {(int64_t)sizeof(ZapPy), ZAP_TD_ATOMIC, 0, NULL};
/* One-object string (#837, D-0145): header + inline byte payload. ATOMIC — the
 * inline bytes carry no pointers, so the collector never scans them and copies
 * the whole object by its stored object size on evacuation. `.size` is the
 * header only (the real size is per-object, via zap_gc_alloc_var). */
const ZapTypeDesc zap_desc_strobj = {(int64_t)sizeof(ZapStr), ZAP_TD_ATOMIC, 0,
                                     NULL};
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
    /* Signature-reflection registry (I#458, D-0089): two more static tables of
     * ZapStr* names, enumerated the same way. */
    ZapParamTable *pt = &zap_param_table;
    if (pt->items) {
        zap_gc_mark(pt->items);
        for (int64_t i = 0; i < pt->len; i++) {
            zap_gc_mark(pt->items[i].owner);
            zap_gc_mark(pt->items[i].name);
            zap_gc_mark(pt->items[i].type_disp);
            zap_gc_mark(pt->items[i].annos);
        }
    }
    ZapSigTable *st = &zap_sig_table;
    if (st->items) {
        zap_gc_mark(st->items);
        for (int64_t i = 0; i < st->len; i++) {
            zap_gc_mark(st->items[i].owner);
            zap_gc_mark(st->items[i].sig);
        }
    }
}

/* Register the precise collector's static roots once at startup. */
__attribute__((constructor)) static void zap_gc_register_statics(void) {
    zap_gc_add_root((void **)&g_cancelled);
    zap_gc_add_root((void **)&zap_current_exc);
    zap_gc_add_root_fn(zap_reflect_mark_roots);
    zap_gc_add_root_fn(zap_scope_wait_mark_roots);
    zap_gc_add_root_fn(zap_netpoll_mark_roots);
    zap_gc_add_root_fn(zap_sig_mark_roots); /* ticket #77: notify subscribers    */
}

/* The precise Immix collector implementation. Included last so it sees every
 * runtime type and the scheduler globals above. */
#include "runtime/gc.inc.c"
#endif

/* Runtime data-race detector hooks (`--race`, #677, epic #665, D-0114). Slice 1
 * no-op stubs, compiled unconditionally for both collectors (the detector is
 * collector-independent). Only a `--race` build calls them; the symbols must
 * link in that case. See zaprace.inc.c. */
#include "runtime/race.inc.c"

/* Numeric acceleration seam (#912, D-0179): the tier/thread knobs + capability
 * probe the array dispatcher (#904) consults. Included after `zap_maxprocs` /
 * `zap_setenv_compat` so it reads them directly. The provider thread-pool call
 * is gated behind ZAP_HAVE_BLAS, which the codegen defines only for a program
 * that links a provider. */
#include "runtime/accel/blas.inc.c"

#include "runtime/math_random.inc.c"
/* statistics (#980, D-0243): descriptive stats over scalar lists. Included
 * after math_random so it reuses the ZapList/ZapStr helpers and libm, and the
 * Shewchuk-sum pattern from `zap_math_fsum`. */
#include "runtime/statistics.inc.c"
/* NumPy-compatible bit-exact PCG64 `Generator` surface (#930/#931, D-0189).
 * Included here, after the ZapArray helpers (array construction/access) and the
 * xoshiro block (`zap_os_entropy_u64`), so both are in scope. */
#include "runtime/accel/random.inc.c"

/* CPU-feature detection (#943, ISA-acceleration epic #139, D-0192). The shared
 * bit-taxonomy header + the malloc-free, compute-once detector every future
 * multiversioning resolver (#944) and arch intrinsic reads. Included here, after
 * the string helpers (`zap_str_new` / `zap_sys_cstr`) its `sys.cpu_*` wrappers
 * build on are in scope. */
#include "runtime/accel/cpu.inc.c"

/* SIMD byte/substring search (#945, ISA-acceleration epic #139, D-0194): the
 * first multiversioned kernel. Included after `zapcpu.inc.c` so its resolver
 * sees `zap_cpu_has`. Defines `zap_memchr_off` / `zap_memmem_off`, which the
 * string/bytes search paths (`zap_str_find_bytes`, forward-declared above) route
 * through. */
#include "runtime/accel/memchr.inc.c"
/* arch.x86_64 / arch.aarch64 intrinsic shims + hash.crc32c (I#139 Tier 2,
 * #962, D-0199). After zapcpu.inc.c so ZapStr and the cpu header are in scope. */
#include "runtime/accel/arch.inc.c"

/* SIMD sort accelerator dispatch (#933, D-0195). After zapcpu.inc.c (for
 * `zap_cpu_has`); the array sort entry points call its two functions via forward
 * declarations. The per-ISA kernels (zapxsort.cpp / zaphwysort.cpp) are linked
 * and ZAP_HAVE_SIMDSORT defined only for a program that reaches the sort path on
 * a supported arch; otherwise this seam returns 0 and the own floor runs. */
#include "runtime/accel/sortaccel.inc.c"

#include "runtime/json.inc.c"
#include "runtime/xml.inc.c"
#include "runtime/url.inc.c"
#include "runtime/collections.inc.c"
#include "runtime/logging_argparse.inc.c"
#include "runtime/csv.inc.c"
#include "runtime/yaml.inc.c"
#include "runtime/zlib.inc.c"
#include "runtime/sync.inc.c"
#include "runtime/net_http.inc.c"
#include "runtime/tls.inc.c"
#include "runtime/crypto_jose.inc.c"
#include "runtime/threading.inc.c"
#include "runtime/profile.inc.c"
