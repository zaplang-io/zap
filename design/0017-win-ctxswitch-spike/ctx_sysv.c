/* Spike I#195: hand-written x86_64 System V context switch (Linux/macOS).
 *
 * Demonstrates replacing ucontext swapcontext() with a ~20-line asm switch that
 * saves/restores the SysV AMD64 callee-saved registers (rbx, rbp, r12-r15) plus
 * rsp and the return address. A "context" is just a single saved stack pointer
 * (void*): the register-save area lives ON the coroutine's own stack, exactly
 * the Boost.Context / libco model.
 *
 * This mirrors Zap's green-stack invariant: the coroutine stack is a plain
 * buffer the runtime owns (here malloc; in zaprt.c a GC_MALLOC object scanned
 * as heap under Boehm). Because the saved registers are pushed onto that same
 * scanned stack, a suspended coroutine's roots-in-registers are covered with no
 * extra work -- the key property native OS Fibers cannot give us.
 *
 * Build/run (host = x86_64 macOS or Linux):
 *   cc -O2 ctx_sysv.c -o ctx_sysv && ./ctx_sysv
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

/* zap_ctx: opaque saved stack pointer. zap_swap(from, to) saves the current
 * callee-saved regs + return addr onto the current stack, stores the resulting
 * rsp into *from, loads rsp from to, pops the saved regs, and returns into the
 * other context. Pure leaf asm, no libc, no syscall (swapcontext does a
 * sigprocmask syscall every call -- this is also strictly faster). */
typedef void *zap_ctx;

/* Defined in the asm block below. */
extern void zap_swap(zap_ctx *from, zap_ctx to);
/* Build an initial context on stack `sp_top` (high addr) that, when first
 * switched to, calls `entry(arg)` on that stack. Returns the initial saved sp. */
extern zap_ctx zap_make(void *sp_top, void (*entry)(void *), void *arg);

__asm__(
    ".text\n"
#if defined(__APPLE__)
    ".globl _zap_swap\n_zap_swap:\n"
#else
    ".globl zap_swap\n.type zap_swap,@function\nzap_swap:\n"
#endif
    /* rdi = from (zap_ctx*), rsi = to (zap_ctx). */
    "  pushq %rbp\n"
    "  pushq %rbx\n"
    "  pushq %r12\n"
    "  pushq %r13\n"
    "  pushq %r14\n"
    "  pushq %r15\n"
    "  movq %rsp, (%rdi)\n"   /* *from = current sp (points at saved regs)      */
    "  movq %rsi, %rsp\n"     /* sp = to                                        */
    "  popq %r15\n"
    "  popq %r14\n"
    "  popq %r13\n"
    "  popq %r12\n"
    "  popq %rbx\n"
    "  popq %rbp\n"
    "  retq\n"                /* returns into the restored context's saved rip  */
);

/* zap_make: lay out a fresh context. We hand-build a frame so the first
 * zap_swap INTO it pops 6 zeroed callee regs and `ret`s into a tiny trampoline
 * that calls entry(arg). arg is parked in r15's slot and moved into rdi by the
 * trampoline. */
__asm__(
    ".text\n"
#if defined(__APPLE__)
    ".globl _zap_trampoline\n_zap_trampoline:\n"
#else
    ".globl zap_trampoline\n.type zap_trampoline,@function\nzap_trampoline:\n"
#endif
    /* On entry: r15 = arg, r14 = entry (set up by zap_make's initial frame). */
    "  movq %r15, %rdi\n"
    "  callq *%r14\n"
    /* entry returned -- in the real runtime the bootstrap switches back to the
     * scheduler here; for the spike we just exit. */
    "  movq $0, %rdi\n"
#if defined(__APPLE__)
    "  callq _exit\n"
#else
    "  callq exit\n"
#endif
);
extern void zap_trampoline(void);

zap_ctx zap_make(void *sp_top, void (*entry)(void *), void *arg) {
    /* 16-byte align the top, then push an initial saved-register frame:
     * layout (high->low): [ret=zap_trampoline][rbp][rbx][r12][r13][r14=entry][r15=arg]
     * zap_swap pops r15,r14,r13,r12,rbx,rbp then `ret`s to zap_trampoline. */
    uintptr_t sp = (uintptr_t)sp_top;
    sp &= ~(uintptr_t)0xF;        /* 16-byte align: trampoline entry gets rsp%16==0 */
    uintptr_t *p = (uintptr_t *)sp;
    *--p = (uintptr_t)zap_trampoline; /* return address for the final `ret`     */
    *--p = 0;                         /* rbp                                    */
    *--p = 0;                         /* rbx                                    */
    *--p = 0;                         /* r12                                    */
    *--p = 0;                         /* r13                                    */
    *--p = (uintptr_t)entry;          /* r14 = entry                            */
    *--p = (uintptr_t)arg;            /* r15 = arg                              */
    return (zap_ctx)p;
}

/* ---- demo: a coroutine that yields back to the scheduler 3 times ---------- */
static zap_ctx sched_ctx;   /* the "scheduler" (main) context                  */
static zap_ctx coro_ctx;    /* the coroutine context                           */
static int step = 0;

static void coro_entry(void *arg) {
    const char *name = (const char *)arg;
    for (int i = 0; i < 3; i++) {
        printf("  [coro %s] running, step %d, local i=%d\n", name, step, i);
        step++;
        zap_swap(&coro_ctx, sched_ctx);  /* yield back to scheduler */
    }
    printf("  [coro %s] done\n", name);
    /* fall through -> trampoline exits; a real runtime switches back instead */
    zap_swap(&coro_ctx, sched_ctx);
}

int main(void) {
    size_t ss = 128 * 1024;
    void *stack = malloc(ss);
    coro_ctx = zap_make((char *)stack + ss, coro_entry, (void *)"A");
    printf("scheduler: resuming coroutine 3 times\n");
    for (int i = 0; i < 3; i++) {
        printf("scheduler: switch #%d (step before=%d)\n", i, step);
        zap_swap(&sched_ctx, coro_ctx);  /* resume coroutine */
        printf("scheduler: back from coroutine (step after=%d)\n", step);
    }
    printf("RESULT: step=%d (expected 3)\n", step);
    free(stack);
    return step == 3 ? 0 : 1;
}
