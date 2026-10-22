/* Spike I#195: hand-written x86_64 Windows context switch (compile-check only).
 *
 * Same single-saved-stack-pointer model as ctx_sysv.c, but adapted to the
 * Windows x64 ABI. Differences from System V that the switch MUST handle:
 *
 *  1. Extra nonvolatile GPRs: Windows adds RDI and RSI to the callee-saved set
 *     {RBX,RBP,RDI,RSI,R12,R13,R14,R15}.
 *  2. Nonvolatile SSE regs: XMM6-XMM15 are callee-saved on Windows (System V
 *     has NONE). That is 10 * 16 = 160 bytes the switch must save/restore.
 *  3. The TEB/TIB stack-bounds fields. Windows keeps the running thread's stack
 *     extent in the TIB (via GS): StackBase (gs:[0x08]), StackLimit
 *     (gs:[0x10]) and DeallocationStack (gs:[0x1478]). SEH unwinding, stack
 *     guard-page growth, and some Win32/CLR calls READ these. When we switch
 *     onto a runtime-owned coroutine stack we must install that stack's bounds
 *     in the TIB and restore the outgoing ones, or the OS believes we are on
 *     the wrong stack (crashes, bad unwinds). System V needs none of this.
 *
 * OFFSETS BELOW ARE THE DOCUMENTED x64 NT_TIB LAYOUT but MUST be re-derived
 * from <winnt.h> on a real toolchain before shipping (A1 implementation).
 *
 * Compile-check on a non-Windows host (object only; cannot link/run here):
 *   clang -target x86_64-pc-windows-msvc -c ctx_win_x64.c -o ctx_win_x64.obj
 */
#include <stdint.h>
#include <stddef.h>

typedef void *zap_ctx;
extern void zap_swap_win(zap_ctx *from, zap_ctx to);

/* Save area pushed on the current stack by zap_swap_win, low->high:
 *   [xmm6..xmm15 (160B)][tib_dealloc][tib_limit][tib_base]
 *   [r15][r14][r13][r12][rsi][rdi][rbx][rbp][ret]
 * A context is the rsp pointing at the bottom (xmm6) of this area. */
__asm__(
    ".text\n"
    ".globl zap_swap_win\n"
    "zap_swap_win:\n"
    /* rcx = from (zap_ctx*), rdx = to (zap_ctx)  [Win x64 arg regs]           */
    "  pushq %rbp\n"
    "  pushq %rbx\n"
    "  pushq %rdi\n"
    "  pushq %rsi\n"
    "  pushq %r12\n"
    "  pushq %r13\n"
    "  pushq %r14\n"
    "  pushq %r15\n"
    /* save TIB stack fields (StackBase, StackLimit, DeallocationStack)        */
    "  movq %gs:0x08, %rax\n  pushq %rax\n"   /* StackBase                     */
    "  movq %gs:0x10, %rax\n  pushq %rax\n"   /* StackLimit                    */
    "  movq %gs:0x1478, %rax\n  pushq %rax\n" /* DeallocationStack             */
    /* save nonvolatile xmm6-xmm15 (160 bytes)                                 */
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
    "  movq %rsp, (%rcx)\n"   /* *from = current sp                            */
    "  movq %rdx, %rsp\n"     /* sp = to                                       */
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
    /* restore TIB stack fields for the incoming context                       */
    "  popq %rax\n  movq %rax, %gs:0x1478\n" /* DeallocationStack              */
    "  popq %rax\n  movq %rax, %gs:0x10\n"   /* StackLimit                     */
    "  popq %rax\n  movq %rax, %gs:0x08\n"   /* StackBase                      */
    "  popq %r15\n"
    "  popq %r14\n"
    "  popq %r13\n"
    "  popq %r12\n"
    "  popq %rsi\n"
    "  popq %rdi\n"
    "  popq %rbx\n"
    "  popq %rbp\n"
    "  retq\n"
);

/* A no-op user of the symbol so the translation unit has an external defn and
 * the object is non-empty; the real zap_make_win would initialise the save area
 * (including the coroutine's TIB StackBase/StackLimit) analogously to the SysV
 * zap_make, which is the part that still needs on-Windows validation. */
void zap_ctx_win_probe(zap_ctx *from, zap_ctx to) { zap_swap_win(from, to); }
