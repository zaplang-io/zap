/* =============================================================================
 * On-Windows verification harness for the A3 signals/preemption/faults port
 * (ticket #197; design/0019). This is the verification the design could not run
 * without a Windows toolchain: it links the REAL backends (`zap_win_a3.c` and the
 * A1 `zap_ctx_win_x64.c`) and checks, on a native windows-latest runner, the four
 * load-bearing mechanisms in isolation — before the full `zaprt.c` compiles on
 * Windows (that still needs A2 #196 + A4 #198):
 *
 *   T1. SEH exception thrown and caught ACROSS frames living on a hand-switched
 *       green stack — the critical A1×A3 interaction (SEH consults the TIB stack
 *       bounds that the A1 switch installs; if they were stale the unwind would
 *       fault). Compiled as C++ so g++ lowers try/throw/catch to exactly the SEH
 *       .pdata/.xdata + __gxx_personality_seh0 + __cxa_* that Zap codegen emits.
 *   T2. Preemption HIJACK is control-flow- and register-faithful: a worker thread
 *       spinning ON a green stack is hijacked many times; the injected call runs
 *       and the loop's register-held checksum is never corrupted.
 *   T3. The bdwgc-STW × monitor SuspendThread race (ticket #197 Q5) is closed by
 *       mutex serialization: a "collector" thread and the preemption monitor both
 *       suspend/resume the worker under a shared lock, stressed hard, with no
 *       deadlock and no corruption of the worker's monotonic progress.
 *   T4. The fault filter reports (type/address/coroutine + dbghelp backtrace) and
 *       lets the process die — tested in a CHILD process (death-by-fault has no
 *       catchable exit, exactly as I#77 tests the POSIX fault handler).
 *   T5. Console control events map to the Signal enum's reachable subset.
 *
 * Build (on Windows, MinGW-w64):
 *   g++ -O2 zap_win_a3_test.cc -ldbghelp -o t && ./t
 * Exit 0 on success; nonzero (with a diagnostic) on any failure. The `--fault`
 * argument puts the process into the T4 child role (deliberate access violation).
 * ========================================================================== */

/* System headers the backends need, pulled in with C++ linkage FIRST so their
 * include guards make the backends' own re-includes (inside the extern "C" block
 * below) no-ops — system headers must not be parsed inside extern "C". */
#include <windows.h>

#include <dbghelp.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

/* The real backends, with C linkage so their symbols match the hand-written asm
 * (which defines unmangled names). ZAP_WIN_A3_TEST exposes the console-mapping
 * accessor so T5 can check it deterministically (without firing a real console
 * event into the process group). */
#define ZAP_WIN_A3_TEST 1
extern "C" {
#include "zap_ctx_win_x64.c"
#include "zap_win_a3.c"
}

static int g_failures = 0;
/* Progress markers: CHECK only prints on failure, so a crash leaves no trace of
 * which test died. MARK prints + flushes a boundary so a segfault localizes to a
 * specific test on the CI log. */
#define MARK(m)                                                                 \
    do {                                                                        \
        fprintf(stderr, "[mark] %s\n", (m));                                    \
        fflush(stderr);                                                         \
    } while (0)
#define CHECK(cond, msg)                                                        \
    do {                                                                        \
        if (!(cond)) {                                                          \
            fprintf(stderr, "FAIL: %s (%s:%d)\n", (msg), __FILE__, __LINE__);   \
            g_failures++;                                                       \
        }                                                                       \
    } while (0)

/* A green stack: a plain heap buffer (the GC-ownership invariant is A1's concern,
 * verified by zap_ctx_win_x64_test.c; here we only need a runtime-owned stack to
 * switch onto). 256 KiB is comfortably above the redzone. */
#define GREEN_STACK_SIZE (256 * 1024)

/* ===========================================================================
 * T1. SEH exception across a hand-switched green stack.
 * ===========================================================================*/
static zap_ctx_t g_t1_main, g_t1_co;
static volatile int g_t1_caught = 0;
static volatile int g_t1_value = 0;
static volatile int g_t1_finally = 0;

struct ZapTestExc {
    int code;
};

/* A few nested frames on the green stack so the unwind walks several .pdata
 * entries, each needing the installed TIB bounds to be consistent. Marked
 * noinline so the frames really exist. */
static __attribute__((noinline)) void t1_depth3(int x) {
    volatile int guard[8];
    for (int i = 0; i < 8; i++)
        guard[i] = x + i; /* touch the green stack */
    if (guard[0] == x)
        throw ZapTestExc{x * 7};
}
static __attribute__((noinline)) void t1_depth2(int x) {
    t1_depth3(x);
    g_t1_value = -999; /* unreachable if the throw propagates */
}
static __attribute__((noinline)) void t1_depth1(int x) { t1_depth2(x); }

/* Runs ON the green stack (entered via zap_ctx_swap). The try/catch here is the
 * analogue of the I#47 spawn landing pad / an in-coroutine `try/except`. Declared
 * extern "C" because its address is passed to the C-linkage `zap_ctx_make` seam
 * (a pointer to a C-linkage function is a distinct type in C++). */
extern "C" void t1_coroutine(void) {
    try {
        t1_depth1(6);
    } catch (const ZapTestExc &e) {
        g_t1_caught = 1;
        g_t1_value = e.code;
    } catch (...) {
        g_t1_caught = 2; /* wrong type reached the catch-all */
    }
    g_t1_finally = 1;
    /* Done: switch back to main; the entry never returns. */
    zap_ctx_swap(&g_t1_co, &g_t1_main);
}

static void test_seh_across_green_stack(void) {
    void *stack = malloc(GREEN_STACK_SIZE);
    CHECK(stack != NULL, "T1 green stack alloc");
    if (!stack)
        return;
    zap_ctx_make(&g_t1_co, stack, GREEN_STACK_SIZE, t1_coroutine);
    zap_ctx_swap(&g_t1_main, &g_t1_co); /* run the coroutine; it swaps back */
    CHECK(g_t1_caught == 1, "T1 exception caught by the correct typed handler");
    CHECK(g_t1_value == 42, "T1 exception value propagated across green-stack frames");
    CHECK(g_t1_finally == 1, "T1 control resumed after the catch on the green stack");
    free(stack);
}

/* ===========================================================================
 * T2 / T3 shared: a worker spinning on a green stack, hijackable by this thread.
 * ===========================================================================*/
struct Worker {
    HANDLE thread;
    void *stack;
    zap_ctx_t native; /* the worker's OS-thread context (to swap back to)       */
    zap_ctx_t green;  /* the spin loop's green-stack context                    */
    volatile LONG64 counter;  /* monotonic progress the loop advances           */
    volatile LONG64 checksum; /* register-carried invariant; corruption => bug  */
    volatile int done;
    volatile int no_preempt; /* mimics g->no_preempt                            */
    uintptr_t lo, hi;        /* green-stack range                               */
};
static Worker g_w;

/* The injected target (stands in for copreempt): in the harness it does NOT
 * switch away — it just records that the hijack fired and returns, so the
 * trampoline restores every volatile and the loop resumes. That is precisely the
 * control-flow/register-fidelity property we are checking. */
static volatile LONG64 g_hijack_count = 0;
/* The per-M in-flight flag (one worker here). zap_win_preempt_try sets it; the
 * target clears it when the preemption is "taken" — the harness stand-in for
 * copreempt clearing cur_m->in_hijack on yield. */
static volatile LONG g_in_hijack = 0;
/* extern "C": assigned to the C-linkage `zap_win_preempt_target` pointer.
 * Deliberately SCRIBBLE every caller-saved (volatile) register after recording
 * the hit: GPRs rax/rcx/rdx/r8-r11 and xmm0-xmm5. The interrupted loop's live
 * volatiles were saved onto the green stack by the hijack trampoline; if the
 * trampoline fails to save/restore ANY of them, this scribble leaks into the
 * resumed computation and the checksum invariant below breaks. This turns T2
 * into a real register-fidelity test, not just a control-flow test. */
extern "C" void t2_target(void) {
    InterlockedIncrement64(&g_hijack_count);
    InterlockedExchange(&g_in_hijack, 0); /* preemption taken: allow the next */
    __asm__ __volatile__(
        "movq $0x1111111111111111, %%rax\n\t"
        "movq $0x2222222222222222, %%rcx\n\t"
        "movq $0x3333333333333333, %%rdx\n\t"
        "movq $0x4444444444444444, %%r8\n\t"
        "movq $0x5555555555555555, %%r9\n\t"
        "movq $0x6666666666666666, %%r10\n\t"
        "movq $0x7777777777777777, %%r11\n\t"
        "pcmpeqd %%xmm0, %%xmm0\n\t"
        "pcmpeqd %%xmm1, %%xmm1\n\t"
        "pcmpeqd %%xmm2, %%xmm2\n\t"
        "pcmpeqd %%xmm3, %%xmm3\n\t"
        "pcmpeqd %%xmm4, %%xmm4\n\t"
        "pcmpeqd %%xmm5, %%xmm5\n\t"
        :
        :
        : "rax", "rcx", "rdx", "r8", "r9", "r10", "r11", "xmm0", "xmm1", "xmm2",
          "xmm3", "xmm4", "xmm5");
}

/* The spin loop. Keeps a checksum in a local (register-allocated under -O2) that
 * must survive every async hijack: counter and checksum advance in lockstep so a
 * clobbered volatile register would desynchronize them. */
extern "C" void t2_spin(void) { /* extern "C": passed to zap_ctx_make */
    LONG64 local = 0;
    while (!g_w.done) {
        local += 1;
        g_w.counter = local;
        g_w.checksum = local * 2654435761LL; /* a value kept across the hijack */
        /* a little work the optimizer can't elide, so the loop spends most time
         * ON the green stack (hijackable) */
        for (int i = 0; i < 64; i++) {
            __asm__ __volatile__("" ::: "memory");
        }
    }
    /* verify our own invariant before leaving */
    if (g_w.checksum != (LONG64)(g_w.counter) * 2654435761LL) {
        g_w.checksum = -1; /* signal corruption */
    }
    zap_ctx_swap(&g_w.green, &g_w.native); /* back to the OS thread */
}

static DWORD WINAPI t2_worker_main(LPVOID) {
    g_w.lo = (uintptr_t)g_w.stack;
    g_w.hi = g_w.lo + GREEN_STACK_SIZE;
    zap_ctx_make(&g_w.green, g_w.stack, GREEN_STACK_SIZE, t2_spin);
    zap_ctx_swap(&g_w.native, &g_w.green); /* run the spin loop; it swaps back */
    return 0;
}

static HANDLE start_worker(void) {
    memset(&g_w, 0, sizeof g_w);
    g_w.stack = malloc(GREEN_STACK_SIZE);
    CHECK(g_w.stack != NULL, "worker green stack alloc");
    HANDLE h = CreateThread(NULL, 0, t2_worker_main, NULL, 0, NULL);
    g_w.thread = h;
    return h;
}

static LONG64 load64(volatile LONG64 *p) {
    return InterlockedCompareExchange64(p, 0, 0);
}

static void test_preempt_hijack(void) {
    zap_win_preempt_target = t2_target;
    g_hijack_count = 0;
    g_in_hijack = 0;
    HANDLE h = start_worker();
    if (!h)
        return;
    /* Wait for the loop to get going (counter advancing on the green stack). */
    for (int i = 0; i < 1000 && load64(&g_w.counter) < 10; i++)
        Sleep(1);
    int hijacked = 0;
    for (int i = 0; i < 200; i++) {
        if (zap_win_preempt_try(h, g_w.lo, g_w.hi, &g_w.no_preempt, &g_in_hijack))
            hijacked++;
        Sleep(1);
    }
    LONG64 c1 = load64(&g_w.counter);
    Sleep(5);
    LONG64 c2 = load64(&g_w.counter);
    g_w.done = 1;
    WaitForSingleObject(h, 5000);
    CHECK(hijacked > 0, "T2 at least one hijack landed on the green stack");
    CHECK(load64(&g_hijack_count) > 0, "T2 injected target actually ran");
    CHECK(c2 > c1, "T2 the loop kept making progress across hijacks");
    CHECK(g_w.checksum != -1, "T2 register checksum intact across every hijack");
    CloseHandle(h);
    free(g_w.stack);
}

/* ===========================================================================
 * T3. Q5: bdwgc-STW × monitor both SuspendThread, serialized by one mutex.
 * ===========================================================================*/
static CRITICAL_SECTION g_stw_mu; /* the serialization lock (zaprt.c: preempt/STW) */
static volatile int g_stw_stop = 0;
static volatile LONG g_stw_deadlock_guard = 0;

/* A fake collector: under the SAME lock, suspend the worker, read its context
 * (as bdwgc STW does to scan it), resume. Hammering this concurrently with the
 * preemption monitor is the exact race Q5 is about. */
static DWORD WINAPI t3_collector(LPVOID) {
    while (!g_stw_stop) {
        EnterCriticalSection(&g_stw_mu);
        if (SuspendThread(g_w.thread) != (DWORD)-1) {
            CONTEXT c;
            memset(&c, 0, sizeof c);
            c.ContextFlags = CONTEXT_CONTROL | CONTEXT_INTEGER;
            GetThreadContext(g_w.thread, &c); /* "scan" */
            ResumeThread(g_w.thread);
        }
        LeaveCriticalSection(&g_stw_mu);
        InterlockedIncrement(&g_stw_deadlock_guard);
    }
    return 0;
}

static void test_stw_suspend_race(void) {
    zap_win_preempt_target = t2_target;
    g_hijack_count = 0;
    g_in_hijack = 0;
    InitializeCriticalSection(&g_stw_mu);
    HANDLE h = start_worker();
    if (!h)
        return;
    for (int i = 0; i < 1000 && load64(&g_w.counter) < 10; i++)
        Sleep(1);
    HANDLE col = CreateThread(NULL, 0, t3_collector, NULL, 0, NULL);
    /* Monitor side: race the collector on SuspendThread under the same lock, for
     * a bounded duration. Each hijack is gated on the worker having ADVANCED its
     * counter since the last one — i.e. it resumed into real loop code and the
     * previous async trampoline has fully returned. This mirrors reality (a
     * coroutine is preempted at most once per ~10 ms slice, and copreempt parks
     * it) and, crucially, prevents the test from re-injecting a trampoline while
     * a prior one is still on the green stack, which would nest frames faster than
     * they unwind and overflow the fixed green stack (a test-cadence artifact, not
     * a hijack-correctness or serialization bug). */
    LONG guard_start = g_stw_deadlock_guard;
    LONG64 hijacks = 0;
    LONG64 last_seen = load64(&g_w.counter);
    ULONGLONG t0 = GetTickCount64();
    while (GetTickCount64() - t0 < 2000 && !g_w.done) {
        /* wait for the worker to make real progress (prior trampoline done) */
        LONG64 c;
        int spins = 0;
        while ((c = load64(&g_w.counter)) == last_seen && spins++ < 1000000 &&
               !g_w.done) {
        }
        last_seen = c;
        EnterCriticalSection(&g_stw_mu);
        if (zap_win_preempt_try(h, g_w.lo, g_w.hi, &g_w.no_preempt, &g_in_hijack))
            hijacks++;
        LeaveCriticalSection(&g_stw_mu);
    }
    CHECK(hijacks > 0, "T3 landed hijacks while the collector raced on suspend");
    /* If the collector is still advancing, there was no deadlock. */
    Sleep(50);
    LONG guard_end = g_stw_deadlock_guard;
    g_stw_stop = 1;
    g_w.done = 1;
    DWORD wc = WaitForSingleObject(col, 5000);
    DWORD ww = WaitForSingleObject(h, 5000);
    CHECK(wc == WAIT_OBJECT_0, "T3 collector thread did not deadlock");
    CHECK(ww == WAIT_OBJECT_0, "T3 worker thread terminated cleanly");
    CHECK(guard_end > guard_start, "T3 collector kept progressing (no lock stall)");
    CHECK(g_w.checksum != -1, "T3 worker invariant intact under concurrent suspend");
    CloseHandle(col);
    CloseHandle(h);
    free(g_w.stack);
    DeleteCriticalSection(&g_stw_mu);
}

/* ===========================================================================
 * T4. Fault filter — exercised in a child process (the fault is terminal).
 * ===========================================================================*/
static void run_fault_child(void) {
    /* Suppress the WER crash dialog so the dying child can never hang CI waiting
     * for a dismiss; also disable the post-filter WER report UI. */
    SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    zap_win_install_fault_filter();
    /* Deliberate null dereference -> EXCEPTION_ACCESS_VIOLATION. The filter
     * reports + returns CONTINUE_SEARCH; the OS terminates us. */
    volatile int *p = (volatile int *)0;
    *p = 1;
    /* Not reached. */
    ExitProcess(0);
}

static void test_fault_filter(void) {
    /* Use the absolute module path, not argv[0]: under the msys2 shell argv[0]
     * can be "./a3test.exe", which CreateProcessA will not resolve. */
    char self[MAX_PATH];
    DWORD got = GetModuleFileNameA(NULL, self, (DWORD)sizeof self);
    CHECK(got > 0 && got < sizeof self, "T4 resolve own module path");
    char cmd[1024];
    snprintf(cmd, sizeof cmd, "\"%s\" --fault", self);
    SECURITY_ATTRIBUTES sa;
    memset(&sa, 0, sizeof sa);
    sa.nLength = sizeof sa;
    sa.bInheritHandle = TRUE;
    HANDLE rd = NULL, wr = NULL;
    CHECK(CreatePipe(&rd, &wr, &sa, 0) != 0, "T4 pipe");
    SetHandleInformation(rd, HANDLE_FLAG_INHERIT, 0);
    STARTUPINFOA si;
    memset(&si, 0, sizeof si);
    si.cb = sizeof si;
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdError = wr;
    si.hStdOutput = wr;
    si.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    PROCESS_INFORMATION pi;
    memset(&pi, 0, sizeof pi);
    BOOL ok = CreateProcessA(NULL, cmd, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi);
    CHECK(ok != 0, "T4 spawn fault child");
    if (!ok) {
        CloseHandle(rd);
        CloseHandle(wr);
        return;
    }
    CloseHandle(wr); /* so the read sees EOF when the child dies */
    char buf[4096];
    DWORD total = 0, n = 0;
    while (total < sizeof(buf) - 1 &&
           ReadFile(rd, buf + total, (DWORD)(sizeof(buf) - 1 - total), &n, NULL) &&
           n > 0) {
        total += n;
    }
    buf[total] = '\0';
    WaitForSingleObject(pi.hProcess, 10000);
    DWORD code = 0;
    GetExitCodeProcess(pi.hProcess, &code);
    CHECK(strstr(buf, "zap fatal") != NULL, "T4 child printed the fault banner");
    CHECK(strstr(buf, "access violation") != NULL, "T4 child named the fault type");
    CHECK(strstr(buf, "backtrace:") != NULL, "T4 child printed a backtrace");
    CHECK(code != 0, "T4 child died with a nonzero status");
    CloseHandle(rd);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
}

/* ===========================================================================
 * T5. Console control event -> Signal enum mapping.
 * ===========================================================================*/
static void test_console_mapping(void) {
    /* Deterministic: check the console-event -> Signal-enum mapping directly. We
     * deliberately do NOT fire a real console event — GenerateConsoleCtrlEvent
     * delivers to the whole process group and could kill the CI shell. */
    CHECK(zap_win_ctrl_to_signo_test(CTRL_C_EVENT) == ZAP_SIGINT,
          "T5 CTRL_C -> SIGINT(2)");
    CHECK(zap_win_ctrl_to_signo_test(CTRL_BREAK_EVENT) == ZAP_SIGQUIT,
          "T5 CTRL_BREAK -> SIGQUIT(3)");
    CHECK(zap_win_ctrl_to_signo_test(CTRL_CLOSE_EVENT) == ZAP_SIGTERM,
          "T5 CTRL_CLOSE -> SIGTERM(15)");
    CHECK(zap_win_ctrl_to_signo_test(CTRL_LOGOFF_EVENT) == ZAP_SIGTERM,
          "T5 CTRL_LOGOFF -> SIGTERM(15)");
    CHECK(zap_win_ctrl_to_signo_test(CTRL_SHUTDOWN_EVENT) == ZAP_SIGTERM,
          "T5 CTRL_SHUTDOWN -> SIGTERM(15)");
    CHECK(zap_win_ctrl_to_signo_test(0xdead) == 0, "T5 unmapped event -> 0 (inert)");
    /* And that registration itself succeeds (a one-call OS wiring). */
    CHECK(zap_win_console_install() == 0, "T5 install console handler");
    zap_win_console_uninstall();
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "--fault") == 0) {
        run_fault_child();
        return 0; /* unreachable */
    }
    MARK("T1 begin (SEH across green stack)");
    test_seh_across_green_stack();
    MARK("T1 end");
    MARK("T2 begin (preempt hijack)");
    test_preempt_hijack();
    MARK("T2 end");
    MARK("T3 begin (Q5 STW x SuspendThread stress)");
    test_stw_suspend_race();
    MARK("T3 end");
    MARK("T4 begin (fault filter child)");
    test_fault_filter();
    MARK("T4 end");
    MARK("T5 begin (console mapping)");
    test_console_mapping();
    MARK("T5 end");
    if (g_failures == 0) {
        fprintf(stderr, "zap_win_a3_test: all checks passed\n");
        return 0;
    }
    fprintf(stderr, "zap_win_a3_test: %d check(s) FAILED\n", g_failures);
    return 1;
}
