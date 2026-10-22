/* =============================================================================
 * Windows x86_64 signals / preemption / fault diagnostics (ticket #197, platform
 * epic #158 / Area A3; design/0019-windows-signals-faults-seh-a3.md).
 *
 * Windows has no POSIX signals, so the three signal-driven subsystems the runtime
 * relies on need Windows-native mechanisms. This file provides the self-contained
 * Windows PRIMITIVES; `zaprt.c` wires them to the scheduler (M/G/copreempt) and
 * the #77 signal-dispatch surface behind the `ZAP_PLATFORM_WINDOWS` seam. Kept in
 * its own translation unit — like the A1 context switch (`zap_ctx_win_x64.c`) —
 * so it both links into the Zap program build (build_executable's Windows arm)
 * AND compiles standalone in the on-Windows verification harness
 * (`zap_win_a3_test.c`), which is how these mechanisms are exercised on a real
 * windows-latest runner before the full `zaprt.c` compiles on Windows (that still
 * needs A2 #196 netpoller + A4 #198 fs/process/random — see the PR body).
 *
 * The three primitives:
 *
 *   1. PREEMPTION (replaces I#172's SIGURG + sigaction, for the conservative
 *      gc-boehm collector — the Windows v1 collector). Immix needs no primitive:
 *      its handler only sets `m->preempt` and the yield is cooperative at the next
 *      `zap_safepoint()`, so on Windows the monitor just stores the flag. Boehm
 *      has no safepoint, so the Unix handler direct-switches; the Windows analogue
 *      is a monitor-driven thread HIJACK: SuspendThread + GetThreadContext, check
 *      the same guards (RSP on the green stack, g->no_preempt == 0), then redirect
 *      RIP to a trampoline that runs `copreempt()` on the coroutine's own green
 *      stack and returns to the interrupted point when the coroutine is resumed.
 *      Modeled on Go's runtime.preemptM for GOOS=windows. The bdwgc-STW × monitor
 *      race (both use SuspendThread) is closed by the CALLER holding a mutex that
 *      the GC's PRE/POST_STOP_WORLD bracket also holds (zaprt.c) — NOT a racy flag
 *      (ticket #197 Q5).
 *
 *   2. FAULTS (replaces I#77's sigaltstack + sigaction(SIGSEGV/FPE/BUS/ILL)).
 *      A last-chance `SetUnhandledExceptionFilter` reports the fault (type, address,
 *      offending coroutine via a hook) + a `dbghelp` StackWalk64 backtrace, then
 *      returns EXCEPTION_CONTINUE_SEARCH so the OS default handler produces the
 *      crash / nonzero exit — the "re-raise with SIG_DFL" analogue. A last-chance
 *      filter (not a first-chance vectored handler) never interferes with normal
 *      C++/SEH exception handling.
 *
 *   3. CONSOLE SIGNALS (replaces the mappable subset of I#77's signal delivery).
 *      `SetConsoleCtrlHandler` maps CTRL_C/BREAK/CLOSE/LOGOFF/SHUTDOWN to the
 *      Zap `Signal` enum's Windows-reachable discriminants (SIGINT/SIGQUIT/SIGTERM)
 *      and delivers via a hook; POSIX-only signals are inert no-ops on Windows.
 *
 * Self-contained: only <windows.h> / <dbghelp.h> (+ freestanding headers). No
 * other runtime headers, exactly like zap_ctx_win_x64.c. `-ldbghelp` is already
 * on the Windows link line (A5 / #199).
 * ========================================================================== */
#include <windows.h>

#include <dbghelp.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

/* The Zap `Signal` prelude enum discriminants that have a Windows console source
 * (crates/syntax/src/parser.rs; the full enum is the Unix signal numbers). Only
 * these three are reachable on Windows; every other discriminant is inert. */
#define ZAP_SIGINT 2  /* CTRL_C_EVENT                                            */
#define ZAP_SIGQUIT 3 /* CTRL_BREAK_EVENT                                        */
#define ZAP_SIGTERM 15 /* CTRL_CLOSE_EVENT / CTRL_LOGOFF_EVENT / CTRL_SHUTDOWN   */

/* ===========================================================================
 * 1. PREEMPTION — thread hijack for the conservative collector (gc-boehm).
 * ===========================================================================*/

/* The function the hijack trampoline calls on the green stack — always the
 * scheduler's `copreempt()`. zaprt.c sets this once at preempt-install time; the
 * standalone harness sets it to its own stub. Read through a global (not passed
 * in a register) so the trampoline clobbers no interrupted register before it has
 * saved every volatile. */
void (*zap_win_preempt_target)(void) = NULL;

/* The hijack trampoline (defined in asm below). Entered when a hijacked worker
 * thread is resumed: its RSP points at the saved original RIP (pushed by the
 * hijack), and RSP ≡ 8 (mod 16) — i.e. the ABI state at a function entry. It
 * saves the FULL volatile register state (an async injection point can be any
 * instruction boundary), calls `zap_win_preempt_target` with correct alignment +
 * 32-byte shadow space, then — when the coroutine is later resumed and control
 * returns here — restores every volatile and `ret`s to the interrupted RIP. The
 * nonvolatile set is preserved across the intervening `zap_ctx_swap` by the swap
 * itself; this trampoline only has to preserve the VOLATILES (GPRs rax/rcx/rdx/
 * r8/r9/r10/r11, EFLAGS, and xmm0-xmm5). */
extern void zap_win_preempt_trampoline(void);

__asm__(".text\n"
        ".globl zap_win_preempt_trampoline\n"
        "zap_win_preempt_trampoline:\n"
        /* entry: [rsp] = orig_rip, rsp%16 == 8.  save volatile GPRs + flags
         * (8 pushes => rsp%16 stays 8). We never touch a register before it is
         * saved, so the interrupted state is wholly preserved. */
        "  pushfq\n"
        "  pushq %rax\n"
        "  pushq %rcx\n"
        "  pushq %rdx\n"
        "  pushq %r8\n"
        "  pushq %r9\n"
        "  pushq %r10\n"
        "  pushq %r11\n"
        /* save volatile xmm0-xmm5 (96 bytes) with movups (no alignment needed) */
        "  subq $96, %rsp\n"
        "  movups %xmm0, 0(%rsp)\n"
        "  movups %xmm1, 16(%rsp)\n"
        "  movups %xmm2, 32(%rsp)\n"
        "  movups %xmm3, 48(%rsp)\n"
        "  movups %xmm4, 64(%rsp)\n"
        "  movups %xmm5, 80(%rsp)\n"
        /* 16-align the call site + 32-byte shadow space. rsp%16==8 here; subq $40
         * => rsp%16==0 at the `call` (so the callee sees rsp%16==8 per ABI). */
        "  subq $40, %rsp\n"
        "  movq zap_win_preempt_target(%rip), %rax\n" /* rax already saved        */
        "  testq %rax, %rax\n"
        "  jz 1f\n"
        "  callq *%rax\n" /* copreempt(): switches away; returns here on resume   */
        "1:\n"
        "  addq $40, %rsp\n"
        "  movups 0(%rsp),  %xmm0\n"
        "  movups 16(%rsp), %xmm1\n"
        "  movups 32(%rsp), %xmm2\n"
        "  movups 48(%rsp), %xmm3\n"
        "  movups 64(%rsp), %xmm4\n"
        "  movups 80(%rsp), %xmm5\n"
        "  addq $96, %rsp\n"
        "  popq %r11\n"
        "  popq %r10\n"
        "  popq %r9\n"
        "  popq %r8\n"
        "  popq %rdx\n"
        "  popq %rcx\n"
        "  popq %rax\n"
        "  popfq\n"
        "  retq\n"); /* pops orig_rip: resume the interrupted instruction stream  */

/* Attempt to preempt the worker thread `hthread` (a HANDLE with
 * THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_SET_CONTEXT access) that is
 * running a coroutine whose green stack is [lo, hi). Returns 1 if it hijacked the
 * thread to run `zap_win_preempt_target` at its next resume, 0 if it declined
 * (and left the thread running) because the thread was not on its green stack or
 * held a runtime lock / allocation (`*no_preempt != 0`).
 *
 * The caller (zaprt.c's monitor) MUST hold the preempt/STW serialization mutex so
 * this never races bdwgc's own stop-the-world SuspendThread (ticket #197 Q5).
 * `no_preempt` points at the live coroutine's `g->no_preempt`; it is re-read AFTER
 * the suspend so a coroutine that entered a critical section between the monitor's
 * pre-check and the suspend is not hijacked mid-lock. */
int zap_win_preempt_try(void *hthread, uintptr_t lo, uintptr_t hi,
                        volatile int *no_preempt) {
    HANDLE th = (HANDLE)hthread;
    if (SuspendThread(th) == (DWORD)-1) {
        return 0;
    }
    CONTEXT ctx;
    memset(&ctx, 0, sizeof ctx);
    ctx.ContextFlags = CONTEXT_CONTROL | CONTEXT_INTEGER;
    if (!GetThreadContext(th, &ctx)) { /* synchronizes the suspend */
        ResumeThread(th);
        return 0;
    }
    uintptr_t rsp = (uintptr_t)ctx.Rsp;
    /* Only hijack a thread actually executing on its coroutine's green stack: the
     * same guard as the Unix handler (not the native scheduler stack mid-switch),
     * plus the re-checked no_preempt (lock/alloc held). */
    if (rsp < lo || rsp >= hi || (no_preempt && *no_preempt != 0)) {
        ResumeThread(th);
        return 0;
    }
    /* Push the interrupted RIP as the trampoline's return address and land the
     * trampoline at RSP ≡ 8 (mod 16) (natural function-entry alignment), wasting
     * 0-8 bytes just below the interrupted RSP (ours until resume; Win x64 has no
     * red zone, but the suspended thread cannot touch this region). */
    uintptr_t sp = rsp - 8;
    sp &= ~(uintptr_t)15; /* 16-align */
    sp += 8;              /* => sp % 16 == 8 */
    *(uintptr_t *)sp = (uintptr_t)ctx.Rip;
    ctx.Rsp = (DWORD64)sp;
    ctx.Rip = (DWORD64)(uintptr_t)&zap_win_preempt_trampoline;
    if (!SetThreadContext(th, &ctx)) {
        ResumeThread(th);
        return 0;
    }
    ResumeThread(th);
    return 1;
}

/* ===========================================================================
 * 2. FAULTS — last-chance diagnostic filter + dbghelp backtrace.
 * ===========================================================================*/

/* zaprt.c installs this to write the offending-coroutine line ("coroutine N
 * [state]\n" or "main/native thread\n") into `buf`; it mirrors the POSIX fault
 * handler's middle line. Default: the standalone case has no scheduler. */
void (*zap_win_fault_describe)(char *buf, size_t n) = NULL;

static const char *zap_win_exc_name(DWORD code) {
    switch (code) {
    case EXCEPTION_ACCESS_VIOLATION:
        return "access violation (SIGSEGV)";
    case EXCEPTION_STACK_OVERFLOW:
        return "stack overflow";
    case EXCEPTION_INT_DIVIDE_BY_ZERO:
        return "integer divide by zero (SIGFPE)";
    case EXCEPTION_FLT_DIVIDE_BY_ZERO:
        return "float divide by zero (SIGFPE)";
    case EXCEPTION_ILLEGAL_INSTRUCTION:
        return "illegal instruction (SIGILL)";
    case EXCEPTION_PRIV_INSTRUCTION:
        return "privileged instruction (SIGILL)";
    case EXCEPTION_IN_PAGE_ERROR:
        return "in-page error (SIGBUS)";
    case EXCEPTION_DATATYPE_MISALIGNMENT:
        return "datatype misalignment (SIGBUS)";
    default:
        return "hardware exception";
    }
}

static LONG WINAPI zap_win_fault_filter(EXCEPTION_POINTERS *ep) {
    EXCEPTION_RECORD *er = ep->ExceptionRecord;
    DWORD code = er->ExceptionCode;
    /* Access violation / in-page error carry the faulting address in
     * ExceptionInformation[1]; others have no meaningful address. */
    void *addr = NULL;
    if ((code == EXCEPTION_ACCESS_VIOLATION || code == EXCEPTION_IN_PAGE_ERROR) &&
        er->NumberParameters >= 2) {
        addr = (void *)er->ExceptionInformation[1];
    }
    char line[256];
    int n = snprintf(line, sizeof line,
                     "\n=== zap fatal: %s (0x%08lx) ===\n"
                     "faulting address: %p\n"
                     "offending coroutine: ",
                     zap_win_exc_name(code), (unsigned long)code, addr);
    if (n > 0) {
        fwrite(line, 1, (size_t)n, stderr);
    }
    if (zap_win_fault_describe) {
        line[0] = '\0';
        zap_win_fault_describe(line, sizeof line);
        fputs(line, stderr);
    } else {
        fputs("main/native thread\n", stderr);
    }

    /* dbghelp backtrace. Sym* is single-threaded; a fault is terminal so we do
     * not contend here. Walk the FAULTING context (ep->ContextRecord), not the
     * filter's own stack. */
    fputs("backtrace:\n", stderr);
    HANDLE proc = GetCurrentProcess();
    HANDLE thr = GetCurrentThread();
    static volatile LONG sym_ready = 0;
    if (InterlockedCompareExchange(&sym_ready, 1, 0) == 0) {
        SymSetOptions(SYMOPT_DEFERRED_LOADS | SYMOPT_UNDNAME | SYMOPT_LOAD_LINES);
        SymInitialize(proc, NULL, TRUE);
    }
    CONTEXT ctx = *ep->ContextRecord;
    STACKFRAME64 sf;
    memset(&sf, 0, sizeof sf);
    sf.AddrPC.Offset = ctx.Rip;
    sf.AddrPC.Mode = AddrModeFlat;
    sf.AddrFrame.Offset = ctx.Rbp;
    sf.AddrFrame.Mode = AddrModeFlat;
    sf.AddrStack.Offset = ctx.Rsp;
    sf.AddrStack.Mode = AddrModeFlat;
    char symbuf[sizeof(SYMBOL_INFO) + 256];
    SYMBOL_INFO *sym = (SYMBOL_INFO *)symbuf;
    for (int i = 0; i < 64; i++) {
        if (!StackWalk64(IMAGE_FILE_MACHINE_AMD64, proc, thr, &sf, &ctx, NULL,
                         SymFunctionTableAccess64, SymGetModuleBase64, NULL)) {
            break;
        }
        DWORD64 pc = sf.AddrPC.Offset;
        if (pc == 0) {
            break;
        }
        memset(sym, 0, sizeof(SYMBOL_INFO));
        sym->SizeOfStruct = sizeof(SYMBOL_INFO);
        sym->MaxNameLen = 255;
        DWORD64 disp = 0;
        if (SymFromAddr(proc, pc, &disp, sym)) {
            fprintf(stderr, "  %2d  %s+0x%llx  [0x%llx]\n", i, sym->Name,
                    (unsigned long long)disp, (unsigned long long)pc);
        } else {
            fprintf(stderr, "  %2d  [0x%llx]\n", i, (unsigned long long)pc);
        }
    }
    fflush(stderr);
    /* Let the OS default handler run: crash dialog / WER / nonzero exit, matching
     * the POSIX handler's "restore SIG_DFL and re-raise". */
    return EXCEPTION_CONTINUE_SEARCH;
}

/* Install the fault diagnostic filter. Also reserve stack headroom so the filter
 * can run even on a near-overflowed native thread (the sigaltstack analogue;
 * green-stack overflow is the A1 soft-redzone zap_panic path, not a hardware
 * fault). Idempotent per process for the filter. */
void zap_win_install_fault_filter(void) {
    SetUnhandledExceptionFilter(zap_win_fault_filter);
    ULONG guarantee = 64 * 1024;
    SetThreadStackGuarantee(&guarantee);
}

/* Call on each worker/monitor thread at startup to give its stack the same
 * fault-handler headroom (SetThreadStackGuarantee is per-thread). */
void zap_win_fault_thread_init(void) {
    ULONG guarantee = 64 * 1024;
    SetThreadStackGuarantee(&guarantee);
}

/* ===========================================================================
 * 3. CONSOLE SIGNALS — map CTRL_* to the Signal enum's reachable subset.
 * ===========================================================================*/

/* zaprt.c installs this to route a delivered signo into the #77 dispatch path
 * (fan-out to notify channels, arm cancel_on, run defaults). Returns 1 if the
 * signal was handled by a user subscription (so the default action is
 * suppressed), 0 otherwise. The harness sets its own recorder. */
int (*zap_win_console_deliver)(int signo) = NULL;

/* Map a console control event to a Zap signal number, or 0 if unmapped. */
static int zap_win_ctrl_to_signo(DWORD ctrl) {
    switch (ctrl) {
    case CTRL_C_EVENT:
        return ZAP_SIGINT;
    case CTRL_BREAK_EVENT:
        return ZAP_SIGQUIT;
    case CTRL_CLOSE_EVENT:
    case CTRL_LOGOFF_EVENT:
    case CTRL_SHUTDOWN_EVENT:
        return ZAP_SIGTERM;
    default:
        return 0;
    }
}

/* The console control handler runs on an OS-spawned thread (NOT a signal
 * context), so it may take locks and signal a condition variable — the whole
 * reason the Windows dispatch path needs no self-pipe + netpoller (A2). */
static BOOL WINAPI zap_win_ctrl_handler(DWORD ctrl) {
    int signo = zap_win_ctrl_to_signo(ctrl);
    if (signo == 0) {
        return FALSE; /* not ours: let the next handler / default run */
    }
    int handled = 0;
    if (zap_win_console_deliver) {
        handled = zap_win_console_deliver(signo);
    }
    /* If a user subscription handled it, swallow the event (suppress the OS
     * default, which for CTRL_C is process termination). Otherwise return FALSE so
     * the default action (terminate) runs — the graceful-shutdown default. */
    return handled ? TRUE : FALSE;
}

/* Install the console control handler (idempotent-safe to call once). */
int zap_win_console_install(void) {
    return SetConsoleCtrlHandler(zap_win_ctrl_handler, TRUE) ? 0 : -1;
}

void zap_win_console_uninstall(void) {
    SetConsoleCtrlHandler(zap_win_ctrl_handler, FALSE);
}

#ifdef ZAP_WIN_A3_TEST
/* Test-only accessor for the console->Signal mapping. Exercising the mapping
 * directly keeps the harness deterministic: GenerateConsoleCtrlEvent delivers to
 * the whole process group (it could kill the CI shell), so the harness must not
 * fire a real console event. Not compiled into the shipped runtime link. */
int zap_win_ctrl_to_signo_test(unsigned long ctrl) {
    return zap_win_ctrl_to_signo((DWORD)ctrl);
}
#endif
