# I#195 spike prototypes (THROWAWAY)

These two standalone C files de-risk the context-switch mechanism recommended in
`design/0017-windows-coroutine-context-switch.md`.
They are **not** part of the build and are **not** production runtime code — they
exist only as reproducible evidence behind the spike recommendation.

- **`ctx_sysv.c`** — hand-written x86_64 System V (Linux/macOS) context switch,
  the same single-saved-stack-pointer model proposed for Windows. Compiles and
  **runs**; the demo resumes a coroutine three times and shows its stack locals
  and callee-saved state surviving each switch.

  ```
  cc -O2 ctx_sysv.c -o ctx_sysv && ./ctx_sysv   # exits 0
  ```

- **`ctx_win_x64.c`** — the Windows x64 variant of the switch: saves the extra
  nonvolatile GPRs (RDI/RSI), the nonvolatile XMM6–XMM15, and swaps the TIB
  stack-bounds fields (StackBase `gs:[0x08]`, StackLimit `gs:[0x10]`,
  DeallocationStack `gs:[0x1478]`). **Compile-checked only** — it assembles to a
  correct COFF object on a non-Windows host, but cannot be linked or run here
  without an MSVC/clang-cl toolchain.

  ```
  clang -target x86_64-pc-windows-msvc -O2 -c ctx_win_x64.c -o ctx_win_x64.obj
  ```

Verified on the dev host (x86_64 macOS, Apple clang 17) on 2026-10-22.
The TIB offsets in `ctx_win_x64.c` are the documented x64 `NT_TIB` layout and
MUST be re-derived from `<winnt.h>` before any real implementation.
