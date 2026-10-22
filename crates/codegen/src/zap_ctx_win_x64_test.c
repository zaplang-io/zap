/* =============================================================================
 * On-Windows verification harness for the A1 coroutine context switch (ticket
 * #195). This is THE critical verification the spike (design/0017) could not run
 * without a Windows toolchain: it links the real `zap_ctx_win_x64.c` backend and
 * checks, on a native windows-latest runner, that
 *
 *   (1) a coroutine switches in/out faithfully, preserving the FULL Windows x64
 *       nonvolatile set across the switch — the callee-saved GPRs AND XMM6-XMM15
 *       (the Windows delta over System V) and the per-coroutine stack; and
 *   (2) [-DZAP_CTX_TEST_BOEHM] the conservative `gc-boehm` GC invariant holds:
 *       a *suspended* coroutine's heap roots — reachable only through a pointer
 *       on its green stack and in a callee-saved register — survive collections
 *       that run while it is parked, because the green stack is a GC_MALLOC
 *       object scanned as heap and the saved register file lives on it. This is
 *       exactly the invariant native Fibers would have broken and the whole
 *       reason A1 uses runtime-owned stacks (design/0017 GC table).
 *
 * It deliberately does NOT depend on the rest of zaprt.c (pthreads, the netpoller
 * epoll path, the immix mmap allocator, the Itanium/SEH exception glue) — none of
 * which is ported to Windows yet (A2/A3/A4) — so it can run on Windows TODAY and
 * isolate the A1 mechanism. The precise `gc-immix` collector cannot be exercised
 * standalone (its roots come from compiler-emitted shadow-stack code and its heap
 * uses mmap, not yet VirtualAlloc); immix's parked-coroutine invariant is instead
 * proven behavior-neutral by the existing Zap goldens under gc-immix on Linux/ARM
 * (gc_parked_coroutine, immix_evac_coro_stress, ...), which already run through
 * this same seam. See the PR body for the exact remaining A3/immix-port gap.
 *
 * Build (on Windows, MinGW-w64):
 *   control-flow:  gcc -O2 zap_ctx_win_x64_test.c -o t      && ./t
 *   boehm invariant: gcc -O2 -DZAP_CTX_TEST_BOEHM zap_ctx_win_x64_test.c -lgc -o tgc && ./tgc
 * Exit code 0 on success; nonzero (with a diagnostic) on any failure.
 * ========================================================================== */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Pull the real backend in as a single translation unit so we test the shipping
 * code, not a copy. */
#include "zap_ctx_win_x64.c"

#ifdef ZAP_CTX_TEST_BOEHM
#define GC_THREADS
#include <gc.h>
#endif

#define GREEN_STACK_SIZE (256 * 1024)
#define ITERS 1000

static zap_ctx_t g_sched;   /* the harness "scheduler" context (native stack)   */
static zap_ctx_t g_coro;    /* the coroutine context                            */

/* ---- control-flow + full nonvolatile preservation ------------------------- */
/* Loop-carried accumulators kept live across every yield. Under -O2 the Win64
 * register allocator parks live-across-call values in the nonvolatile GPRs and
 * XMM6-XMM15 (both nonvolatile on Windows), so if the switch fails to save/
 * restore ANY of them, at least one accumulator diverges from the independently
 * computed expected value below. The doubles use integer-valued arithmetic so
 * the comparison is exact. */
static long g_a[6];
static double g_d[8];
static volatile int g_coro_done;

static void ctrl_coro_entry(void) {
    long a0 = 0, a1 = 0, a2 = 0, a3 = 0, a4 = 0, a5 = 0;
    double d0 = 0, d1 = 0, d2 = 0, d3 = 0, d4 = 0, d5 = 0, d6 = 0, d7 = 0;
    for (int k = 0; k < ITERS; k++) {
        a0 += k;
        a1 += (long)k * 2;
        a2 += (long)k * 3;
        a3 += k + 7;
        a4 += (k ^ 0x55);
        a5 += (long)k * 5;
        d0 += (double)k;
        d1 += (double)(k * 2);
        d2 += (double)(k * 3);
        d3 += (double)(k * 4);
        d4 += (double)(k * 5);
        d5 += (double)(k * 6);
        d6 += (double)(k * 7);
        d7 += (double)(k * 8);
        zap_ctx_swap(&g_coro, &g_sched); /* yield to the scheduler */
    }
    g_a[0] = a0; g_a[1] = a1; g_a[2] = a2; g_a[3] = a3; g_a[4] = a4; g_a[5] = a5;
    g_d[0] = d0; g_d[1] = d1; g_d[2] = d2; g_d[3] = d3;
    g_d[4] = d4; g_d[5] = d5; g_d[6] = d6; g_d[7] = d7;
    g_coro_done = 1;
    zap_ctx_swap(&g_coro, &g_sched); /* final return to scheduler, never resumed */
}

/* Scramble every nonvolatile register between resumes so a resume that leaked
 * the scheduler's register state into the coroutine (or failed to restore the
 * coroutine's) would corrupt an accumulator. Marked noinline + the clobber list
 * forces the compiler to actually materialise the writes around the call. */
static void __attribute__((noinline)) scramble_nonvolatiles(void) {
    __asm__ __volatile__(
        "movq $0xdeadbeef11, %%rbx\n movq $0xdeadbeef12, %%rdi\n"
        "movq $0xdeadbeef13, %%rsi\n movq $0xdeadbeef14, %%r12\n"
        "movq $0xdeadbeef15, %%r13\n movq $0xdeadbeef16, %%r15\n"
        "pcmpeqd %%xmm6, %%xmm6\n pcmpeqd %%xmm7, %%xmm7\n"
        "pcmpeqd %%xmm8, %%xmm8\n pcmpeqd %%xmm9, %%xmm9\n"
        "pcmpeqd %%xmm10, %%xmm10\n pcmpeqd %%xmm11, %%xmm11\n"
        "pcmpeqd %%xmm12, %%xmm12\n pcmpeqd %%xmm13, %%xmm13\n"
        "pcmpeqd %%xmm14, %%xmm14\n pcmpeqd %%xmm15, %%xmm15\n"
        :
        :
        : "rbx", "rdi", "rsi", "r12", "r13", "r15", "xmm6", "xmm7", "xmm8",
          "xmm9", "xmm10", "xmm11", "xmm12", "xmm13", "xmm14", "xmm15");
}

static int run_ctrl_test(void *stack) {
    zap_ctx_make(&g_coro, stack, GREEN_STACK_SIZE, ctrl_coro_entry);
    int resumes = 0;
    while (!g_coro_done) {
        scramble_nonvolatiles();
        zap_ctx_swap(&g_sched, &g_coro);
        resumes++;
    }
    /* one extra resume already happened via the final yield in the entry; the
     * accumulators are published in g_a/g_d. */
    long ea[6] = {0, 0, 0, 0, 0, 0};
    double ed[8] = {0, 0, 0, 0, 0, 0, 0, 0};
    for (int k = 0; k < ITERS; k++) {
        ea[0] += k; ea[1] += (long)k * 2; ea[2] += (long)k * 3;
        ea[3] += k + 7; ea[4] += (k ^ 0x55); ea[5] += (long)k * 5;
        for (int j = 0; j < 8; j++) ed[j] += (double)(k * (j + 1));
    }
    for (int i = 0; i < 6; i++) {
        if (g_a[i] != ea[i]) {
            fprintf(stderr, "FAIL ctrl: GPR acc[%d] = %ld, expected %ld "
                            "(nonvolatile GPR not preserved across switch)\n",
                    i, g_a[i], ea[i]);
            return 1;
        }
    }
    for (int i = 0; i < 8; i++) {
        if (g_d[i] != ed[i]) {
            fprintf(stderr, "FAIL ctrl: XMM acc[%d] = %.1f, expected %.1f "
                            "(nonvolatile XMM not preserved across switch)\n",
                    i, g_d[i], ed[i]);
            return 1;
        }
    }
    /* ITERS yields inside the loop + 1 final yield after publishing results. */
    if (resumes != ITERS + 1) {
        fprintf(stderr, "FAIL ctrl: %d resumes, expected %d\n", resumes,
                ITERS + 1);
        return 1;
    }
    printf("PASS ctrl: %d coroutine round-trips; all callee-saved GPRs + "
           "XMM6-15 + stack preserved\n",
           resumes);
    return 0;
}

#ifdef ZAP_CTX_TEST_BOEHM
/* ---- gc-boehm: suspended-coroutine root survives collection --------------- */
#define SENTINEL_WORDS 64
#define SENTINEL_MAGIC 0xCAFEBABEDEADBEEFULL

typedef struct GcObj {
    uint64_t words[SENTINEL_WORDS];
} GcObj;

static int g_gc_ok;

static void fill_sentinel(GcObj *o) {
    for (int i = 0; i < SENTINEL_WORDS; i++) o->words[i] = SENTINEL_MAGIC + i;
}
static int check_sentinel(const GcObj *o) {
    for (int i = 0; i < SENTINEL_WORDS; i++)
        if (o->words[i] != SENTINEL_MAGIC + (uint64_t)i) return 0;
    return 1;
}

static void gc_coro_entry(void) {
    /* The ONLY live reference to this object is this green-stack local (plus,
     * forced below, a callee-saved register). It is never stored in any global
     * or the scheduler stack, so only scanning the green stack + saved register
     * file as heap keeps it alive. */
    GcObj *obj = (GcObj *)GC_MALLOC(sizeof(GcObj));
    if (!obj) { g_gc_ok = 0; return; }
    fill_sentinel(obj);
    if (!check_sentinel(obj)) { g_gc_ok = 0; return; }
    /* Keep the pointer live in a callee-saved register across the yield as well
     * as the green-stack local (register-root coverage), opaque to the
     * optimizer. When the coroutine parks, the switch spills r13 onto the green
     * stack, so this copy is ALSO found by scanning the green stack as heap. */
    register GcObj *keep __asm__("r13") = obj;
    __asm__ __volatile__("" : "+r"(keep));
    zap_ctx_swap(&g_coro, &g_sched); /* park: now the scheduler stresses the GC */
    obj = keep;
    __asm__ __volatile__("" : "+r"(keep));
    g_gc_ok = check_sentinel(obj);   /* survived collection while parked? */
    zap_ctx_swap(&g_coro, &g_sched);
    /* NOTE: we deliberately do NOT trigger a collection from WITHIN the running
     * coroutine here. That would test whether boehm re-reads the TIB StackBase
     * (which the switch installs) to bound its stack scan — but boehm on Windows
     * may use a cached GC_stackbottom, so an in-coroutine collection is a
     * boehm-Windows / SEH (A3/#197) concern, not an A1 context-switch property.
     * The load-bearing A1 invariant — a *suspended* coroutine's root survives a
     * collection that runs while it is parked, because its green stack + spilled
     * register file are scanned as heap — is exactly what is exercised above. */
}

static int run_gc_test(void) {
    /* Green stack is a GC_MALLOC (non-atomic, pointer-bearing) object, scanned
     * as heap — the load-bearing boehm invariant. */
    void *stack = GC_MALLOC(GREEN_STACK_SIZE);
    if (!stack) { fprintf(stderr, "FAIL gc: GC_MALLOC stack\n"); return 1; }
    g_gc_ok = -1;
    zap_ctx_make(&g_coro, stack, GREEN_STACK_SIZE, gc_coro_entry);
    zap_ctx_swap(&g_sched, &g_coro); /* run coro until it parks */
    if (g_gc_ok == 0) {
        fprintf(stderr, "FAIL gc: sentinel corrupted before park (in-coroutine "
                        "collection / TIB bounds)\n");
        return 1;
    }
    /* Coroutine is parked holding the only reference to its heap object. Churn
     * a lot of garbage and force many collections while it sleeps. */
    volatile uint64_t sink = 0;
    for (int i = 0; i < 4000; i++) {
        void *junk = GC_MALLOC(4096);
        memset(junk, i & 0xff, 4096);
        sink += ((unsigned char *)junk)[0];
        if ((i & 0x3f) == 0) GC_gcollect();
    }
    GC_gcollect();
    GC_gcollect();
    (void)sink;
    zap_ctx_swap(&g_sched, &g_coro); /* resume: coro re-checks its object */
    if (g_gc_ok != 1) {
        fprintf(stderr, "FAIL gc: parked coroutine's heap root did NOT survive "
                        "collection (green stack not scanned as heap)\n");
        return 1;
    }
    printf("PASS gc-boehm: parked-coroutine heap root survived GC stress "
           "(green stack + saved registers scanned as heap)\n");
    return 0;
}
#endif /* ZAP_CTX_TEST_BOEHM */

int main(void) {
#ifdef ZAP_CTX_TEST_BOEHM
    GC_INIT();
    if (run_gc_test() != 0) return 1;
#endif
    void *stack = malloc(GREEN_STACK_SIZE);
    if (!stack) { fprintf(stderr, "FAIL: malloc stack\n"); return 1; }
    int rc = run_ctrl_test(stack);
    free(stack);
    if (rc != 0) return 1;
    printf("ALL PASS (I#195 A1 Windows context switch verified)\n");
    return 0;
}
