---
title: sys.getpass([prompt]) — terminal-safe hidden line input; typed OSError when stdin is not a terminal
status: Accepted
issue: 1684
pr:
date: 2026-11-27
domain: stdlib
supersedes:
superseded-by:
---

# `sys.getpass([prompt])` hidden line input

**Status:** Accepted.

## Decision

Add a hidden (no-echo) line-input primitive:

```zap
str secret = sys.getpass("Password: ")
str secret2 = sys.getpass()            # prompt defaults to "Password: "
```

- Signature `sys.getpass(prompt: str = "Password: ") -> str`. `prompt` is
  positional-or-keyword (`sys.getpass(prompt=...)`); the default is filled by
  the lowerer so the runtime always receives the string to display.
- It reads **one line from stdin (fd 0)** with terminal echo suppressed and
  writes the prompt to **stderr (fd 2)** — never stdout — so a program whose
  stdout is captured (`pw=$(prog)`) still shows the prompt and never captures
  it. The returned string excludes the trailing line ending (and a preceding
  `\r`), exactly like `input()`.
- **stdin must be a terminal.** When it is not (a pipe or a redirected file),
  `getpass` raises a catchable **`OSError`** *before* writing the prompt or
  reading any byte — there is no silent fallback to an echoing read, so a secret
  is never consumed or disclosed on the wrong channel.
- **End-of-input with no bytes read raises `EOFError`**, matching `input()`.
- The **original terminal state is restored on every exit** — a normal return,
  EOF, or an I/O error after echo was disabled. A terminal that already had echo
  off stays off (save → modify → restore). Echo is disabled with POSIX
  `tcsetattr(TCSAFLUSH)` (clearing `ECHO`, setting `ECHONL` so the Enter newline
  still advances the cursor) / Windows `SetConsoleMode` (clearing
  `ENABLE_ECHO_INPUT`), draining type-ahead before the prompt so characters
  typed while echo was still on are discarded rather than read back visibly.
- **No secret is ever written to stdout/stderr or placed in diagnostic text:**
  typed bytes are never echoed, and the failure messages are fixed strings
  ("stdin is not a terminal", "EOF when reading a password").

## Rationale

### Surface: a `sys.getpass` seam function, not `input(hide_input=True)`

The issue offered two shapes: a `hide_input=True` keyword on the `input()`
builtin, or a dedicated `sys.getpass(prompt)` helper. **`sys.getpass` is
chosen** because:

- `input()` is a positional-only *builtin* (not a seam module function); it has
  no keyword-argument plumbing, so `hide_input=True` would mean teaching the
  builtin layer kwargs for a single flag.
- `sys.getpass` lives beside the existing stdio surface (`sys.std{in,out,err}_fd`,
  `sys.stdin/stdout/stderr`) and maps directly onto Python's `getpass.getpass`,
  the reference API, keeping the non-core CLI library (zaplang-io/cli#11) close
  to Click/`getpass`.
- It keeps visible `input()` semantics completely untouched, as the acceptance
  requires.

### Error policy: fail typed, never fall back to an echoing read

CPython's `getpass` falls back to a warning and an echoing read when no tty is
available. The issue explicitly rejects that ("do not silently fall back to an
echoing read"). So Zap **deliberately diverges from CPython here**: a
non-terminal stdin is an `OSError`, raised before the prompt or any read, so the
caller decides what to do and no secret is ever echoed or consumed by accident.
`OSError` (not a bespoke error type) is used because "stdin is not a terminal"
is an I/O/OS condition and `OSError` is already the catchable base the rest of
the fd surface raises; adding a new exception id for one call was not worth it.

### Reading stdin / prompting stderr, and the restore guarantee

Reading fd 0 and requiring it to be a tty makes the pipe→failure contract
deterministic and testable (a pipe is reliably "not a terminal"), which the
acceptance's "a pipe … follows the documented typed failure path before
consuming" demands. The prompt goes to stderr to keep stdout clean for capture.
The read is performed by a **non-raising** helper that returns a status; the
caller restores the terminal and only *then* turns a non-OK status into a raise,
so echo is never left disabled across the `longjmp` a raise performs.

## Consequences

- Blocks-unblocked: the hidden-input portion of zaplang-io/cli#11.
- Behavioral divergence from CPython `getpass` (which echoes on a non-tty) is
  intentional and documented; it is noted in `docs/PYTHON_DIFFERENCES.md`.
- Pairs with [stdlib/1685-isatty-terminal-detection](1685-isatty-terminal-detection.md).
- **Known limitation:** a hard terminating signal (an uncaught SIGINT) delivered
  while the read is blocked can still leave echo disabled, because restoring the
  terminal across an async terminating signal is a core signals concern (a tty
  SIGINT also typically reaches the foreground process group, which Zap's signal
  model, #77, governs). `EINTR` on the read itself is retried, and every path
  this function controls — return, EOF, I/O error — restores the terminal. A
  terminal `reset`/shell commonly repairs echo after such a kill.
