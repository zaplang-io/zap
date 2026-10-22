/* =============================================================================
 * Windows x86_64 coroutine context switch (ticket #195, platform epic #158 /
 * Area A1; design/0017-windows-coroutine-context-switch.md).
 *
 * This is the hand-written assembly backend for the `zap_ctx_swap`/`zap_ctx_make`
 * seam declared in `zaprt.c` (the POSIX side uses `ucontext` behind the same
 * seam; see the seam comment in zaprt.c). It replaces the one load-bearing POSIX
 * dependency — stack switching — on Windows, where `ucontext` does not exist.
 *
 * The model is the Boost.Context / libco "a context is a single saved stack
 * pointer" model, kept BYTE-COMPATIBLE with the GC invariant Zap relies on: the
 * green stack is a runtime-owned buffer (a `GC_MALLOC` object scanned as heap
 * under the conservative `gc-boehm` fallback), and the saved register file lives
 * ON that same stack, so a *suspended* coroutine's callee-saved roots are found
 * with no stack maps and no safepoints — exactly as the POSIX `ucontext_t`-in-G
 * arrangement did, and exactly what native Windows Fibers could NOT give us
 * (their stack + suspended register file are OS-opaque — see the spike's GC
 * table). Under the precise `gc-immix` collector the machine stack is never
 * scanned and roots come from the per-coroutine shadow chain, so this backend is
 * orthogonal to immix root-finding; it only has to resume control flow faithfully.
 *
 * Windows x64 ABI specifics this switch handles (vs System V — the deltas that
 * make Windows its own backend, design/0017 §"Windows x64 ABI specifics"):
 *   1. Extra nonvolatile GPRs: Windows adds RDI and RSI to the callee-saved set
 *      {RBX,RBP,RDI,RSI,R12,R13,R14,R15}.
 *   2. Nonvolatile SSE regs: XMM6-XMM15 are callee-saved on Windows (System V
 *      has none) — 10 * 16 = 160 bytes the switch must save/restore with movaps.
 *   3. TIB stack-bounds fields, read via GS: StackBase gs:[0x08], StackLimit
 *      gs:[0x10], DeallocationStack gs:[0x1478]. SEH unwinding, stack guard-page
 *      growth, and some Win32 calls read these; the conservative GC's own stack
 *      scan on Windows reads StackBase to bound the walk. The switch installs the
 *      incoming coroutine's bounds and restores the outgoing thread's, so an
 *      allocation (and a future SEH unwind, A3/#197) that happens while running
 *      on a green stack sees consistent bounds. System V needs none of this.
 *   4. Shadow space / 16-byte alignment: `zap_ctx_make` lays out the initial
 *      frame so the trampoline enters the user entry with a 32-byte home/shadow
 *      area and 16-byte-aligned rsp at the call site.
 *
 * Guard pages are deliberately NOT used (v1): a PROT_NONE / guard page inside a
 * GC-scanned buffer would break conservative boehm scanning (noted for Linux
 * too). Overflow is a soft redzone check at park points (ZAP_STACK_REDZONE in
 * zaprt.c); the TIB StackLimit we install is simply the low end of the fixed
 * buffer. Hardware/growable stacks remain deferred to the precise-GC era.
 *
 * SEH / exception unwinding across a green stack is A3/#197 (not A1): this file
 * provides no `.pdata`/`.xdata` unwind info for the switch itself, so an unwind
 * must not cross `zap_ctx_swap`. That is already the design — the I#47 spawn
 * landing pad is each coroutine's base catch frame and unwinding never crosses
 * the bootstrap into the scheduler. The TIB maintenance here is the groundwork
 * that lets A3 make SEH work; A1 only needs control-flow-faithful switching.
 *
 * This translation unit is self-contained (no other runtime headers) so it both
 * links into the Zap program build (wired into build_executable's Windows arm,
 * crates/codegen/src/lib.rs) AND compiles standalone in the on-Windows context-
 * switch + GC verification harness (zap_ctx_win_x64_test.c), which is the only
 * way the A1 GC invariant can be exercised on Windows before the full runtime
 * compiles there (that needs A3 exceptions + the immix mmap->VirtualAlloc port).
 * ========================================================================== */
#include <stddef.h>
#include <stdint.h>

/* MUST match the `zap_ctx_t` declared in zaprt.c's Windows branch: a single
 * saved stack pointer. The register save-area lives on the coroutine's own
 * (runtime-owned, boehm-scanned) green stack, not here. */
typedef struct zap_ctx_t {
    void *sp;
} zap_ctx_t;

/* C-visible prototypes for the two seam entry points (identical to zaprt.c's
 * Windows-branch declarations). `zap_ctx_swap` is defined in the asm block
 * below; `zap_ctx_make` is the C function at the bottom of this file. */
void zap_ctx_swap(zap_ctx_t *from, zap_ctx_t *to);
void zap_ctx_make(zap_ctx_t *c, void *stack, size_t stack_size,
                  void (*entry)(void));

/* The trampoline the freshly-made context first `ret`s into (defined in asm
 * below). It calls the user entry (held in r14 by the initial frame). */
extern void zap_ctx_trampoline_win(void);

/* ---------------------------------------------------------------------------
 * zap_ctx_swap(from, to): save the full Windows x64 nonvolatile set + the TIB
 * stack bounds onto the current stack, store the resulting rsp into *from, load
 * rsp from *to, restore its saved set, and `ret` into its saved return address.
 *
 * Saved frame, from the saved rsp (low) upward — `zap_ctx_make` builds an
 * identical frame for a fresh coroutine so the first swap-in resumes the same way:
 *   [rsp+0  .. +144]  xmm6..xmm15            (160 bytes, 16-byte aligned)
 *   [rsp+160]         DeallocationStack      (TIB gs:[0x1478])
 *   [rsp+168]         StackLimit             (TIB gs:[0x10])
 *   [rsp+176]         StackBase              (TIB gs:[0x08])
 *   [rsp+184]         r15
 *   [rsp+192]         r14    (= user entry for a made context)
 *   [rsp+200]         r13
 *   [rsp+208]         r12
 *   [rsp+216]         rsi
 *   [rsp+224]         rdi
 *   [rsp+232]         rbx
 *   [rsp+240]         rbp
 *   [rsp+248]         return address (= trampoline for a made context)
 * Total 256 bytes; the saved rsp is 16-byte aligned (movaps requires it).
 * Win x64 arg regs: rcx = from (zap_ctx_t*), rdx = to (zap_ctx_t*).
 * ------------------------------------------------------------------------- */
__asm__(
    ".text\n"
    ".globl zap_ctx_swap\n"
    "zap_ctx_swap:\n"
    "  pushq %rbp\n"
    "  pushq %rbx\n"
    "  pushq %rdi\n"
    "  pushq %rsi\n"
    "  pushq %r12\n"
    "  pushq %r13\n"
    "  pushq %r14\n"
    "  pushq %r15\n"
    /* save outgoing TIB stack bounds (high..low push order => StackBase highest) */
    "  movq %gs:0x08, %rax\n  pushq %rax\n"   /* StackBase                        */
    "  movq %gs:0x10, %rax\n  pushq %rax\n"   /* StackLimit                       */
    "  movq %gs:0x1478, %rax\n  pushq %rax\n" /* DeallocationStack                */
    /* save nonvolatile xmm6-xmm15 (160 bytes); rsp is 16-aligned here           */
    "  subq $160, %rsp\n"
    "  movaps %xmm6,  0(%rsp)\n"
    "  movaps %xmm7,  16(%rsp)\n"
    "  movaps %xmm8,  32(%rsp)\n"
    "  movaps %xmm9,  48(%rsp)\n"
    "  movaps %xmm10, 64(%rsp)\n"
    "  movaps %xmm11, 80(%rsp)\n"
    "  movaps %xmm12, 96(%rsp)\n"
    "  movaps %xmm13, 112(%rsp)\n"
    "  movaps %xmm14, 128(%rsp)\n"
    "  movaps %xmm15, 144(%rsp)\n"
    /* swap stacks */
    "  movq %rsp, (%rcx)\n" /* from->sp = current rsp (points at saved xmm6)      */
    "  movq (%rdx), %rsp\n" /* rsp = to->sp                                       */
    /* restore nonvolatile xmm */
    "  movaps 0(%rsp),   %xmm6\n"
    "  movaps 16(%rsp),  %xmm7\n"
    "  movaps 32(%rsp),  %xmm8\n"
    "  movaps 48(%rsp),  %xmm9\n"
    "  movaps 64(%rsp),  %xmm10\n"
    "  movaps 80(%rsp),  %xmm11\n"
    "  movaps 96(%rsp),  %xmm12\n"
    "  movaps 112(%rsp), %xmm13\n"
    "  movaps 128(%rsp), %xmm14\n"
    "  movaps 144(%rsp), %xmm15\n"
    "  addq $160, %rsp\n"
    /* install incoming TIB stack bounds (reverse of the save push order)        */
    "  popq %rax\n  movq %rax, %gs:0x1478\n" /* DeallocationStack                 */
    "  popq %rax\n  movq %rax, %gs:0x10\n"   /* StackLimit                        */
    "  popq %rax\n  movq %rax, %gs:0x08\n"   /* StackBase                         */
    "  popq %r15\n"
    "  popq %r14\n"
    "  popq %r13\n"
    "  popq %r12\n"
    "  popq %rsi\n"
    "  popq %rdi\n"
    "  popq %rbx\n"
    "  popq %rbp\n"
    "  retq\n");

/* ---------------------------------------------------------------------------
 * zap_ctx_trampoline_win: entered via the first swap-in's `ret`. The user entry
 * is in r14 (restored from the made frame's r14 slot). rsp is the 16-byte
 * aligned top of the coroutine stack. Establish a 32-byte shadow/home area at a
 * 16-byte-aligned rsp (so the call site is aligned and the callee's rsp%16==8 on
 * entry, as the ABI requires) and call entry; entry never returns (the bootstrap
 * switches back to the scheduler), so control never falls through the `ud2`.
 * ------------------------------------------------------------------------- */
__asm__(
    ".text\n"
    ".globl zap_ctx_trampoline_win\n"
    "zap_ctx_trampoline_win:\n"
    "  andq $-16, %rsp\n" /* defensive 16-byte align (top is already aligned)     */
    "  subq $32, %rsp\n"  /* 32-byte home/shadow space for the call               */
    "  callq *%r14\n"     /* entry()  [void(*)(void)] — never returns             */
    "  ud2\n");

/* ---------------------------------------------------------------------------
 * zap_ctx_make(c, stack, stack_size, entry): lay out an initial frame on the
 * buffer [stack, stack+stack_size) identical to one zap_ctx_swap would save, so
 * the first swap-in restores a zeroed register set, installs this stack's TIB
 * bounds, and `ret`s into the trampoline (which calls entry on this stack).
 * `stack` is the LOW address (the GC_MALLOC base); `entry` is void(*)(void)
 * (g_bootstrap in the runtime), matching the POSIX makecontext(c, entry, 0).
 * ------------------------------------------------------------------------- */
void zap_ctx_make(zap_ctx_t *c, void *stack, size_t stack_size,
                  void (*entry)(void)) {
    uintptr_t base = (uintptr_t)stack;
    uintptr_t top = (base + stack_size) & ~(uintptr_t)0xF; /* 16-align high end   */
    uintptr_t sp = top - 256;                              /* 16-aligned frame    */
    uintptr_t *f = (uintptr_t *)sp;
    for (size_t i = 0; i < 256 / sizeof(uintptr_t); i++) {
        f[i] = 0; /* zero xmm6-15 slots + all saved GPRs                          */
    }
    f[160 / 8] = base; /* DeallocationStack = low end of the fixed buffer         */
    f[168 / 8] = base; /* StackLimit        = low end                            */
    f[176 / 8] = top;  /* StackBase         = high end (aligned)                  */
    f[192 / 8] = (uintptr_t)entry;                   /* r14 = user entry          */
    f[248 / 8] = (uintptr_t)zap_ctx_trampoline_win;  /* return addr -> trampoline */
    c->sp = (void *)sp;
}
