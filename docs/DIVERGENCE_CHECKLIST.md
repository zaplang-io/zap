# Python-divergence checklist (epic #374, I#376)

Zap is "Python-readable": over the subset the two languages share, Zap intends
Python's runtime semantics — **except** where a locked decision says otherwise,
and those exceptions must be *deliberate and pinned*, never accidental. We kept
finding semantic divergences only after features merged, when a downstream
package tripped on them (`and`/`or` short-circuit #334, dict iteration #299,
`dict.get` #281, floored `%` #381, chained comparison #386, empty-string
truthiness #385, container truthiness #382, built-in-fault catchability #383).
Each was a case where the obvious Python reader's expectation and the shipped
behavior silently disagreed.

This checklist is the cheap net that catches them at PR time. **It is required
for every language-feature PR and every `D-00xx` decision** (see
`CONTRIBUTING.md` and the PR template). Keep it lightweight: it is a prompt to
*think like a Python reader for five minutes*, not paperwork.

## The checklist

For the behavior your change introduces or touches, answer each question. For
every "differs" answer, either fix it to match Python **or** record it as an
intentional divergence (a `D-00xx` line + a conformance case that pins it).

1. **Truthiness.** Does your feature ever put a value in a boolean context
   (`if`, `while`, `and`/`or`, `not`, a comprehension `if`)? Then: are empty
   containers, `""`, `0`, `0.0`, and `None` **falsy**, and everything else
   truthy — the same as Python? (TRUTH-*, D-0022.)
2. **Short-circuit.** If `and`/`or` are involved, is the right operand evaluated
   **only** on the deciding path (no side effect when short-circuited)? And is
   the *result value* what Zap specifies — a strict `bool`, **not** the
   surviving operand as Python returns (a deliberate divergence, D-0022)?
   (BOOL-1, BOOL-2.)
3. **Evaluate-once.** Is every operand/argument/subscript evaluated **exactly
   once**? Watch chained comparison (`a < f() < b`, CMP-2), compound assignment,
   `with`, default arguments.
4. **Default arguments.** Any default value a per-call fresh value (Zap, D-0038),
   **not** Python's once-at-def shared object? A mutable default (`[]`, `{}`)
   must not accumulate across calls. (FN-1.)
5. **Numeric edges.** For `/` `//` `%` `**` and casts: `/` is float true
   division (integral results keep `.0`), `//` floors toward −∞, `%` takes the
   sign of the divisor, and the identity `a == (a // b) * b + (a % b)` holds —
   on **negative** operands too. Does default-`int` arithmetic **wrap** at the
   int64 boundary (Zap divergence, not Python bigint)? (NUM-*.)
6. **Empty / negative / boundary.** Did you test the empty input, a negative
   index/length, a zero divisor, the min/max integer, an absent dict key, and a
   one-element and zero-element collection? These are where parity breaks.
7. **Ordering.** For anything built on `dict`/`set`: is every observation
   **order-independent** (iteration order is unspecified, D-0056)? Do not rely
   on insertion order unless the type is the opt-in ordered one.
8. **Comparison & equality.** Is `==` content/structural as specified, is string
   ordering lexicographic by codepoint, and does chained comparison mean
   `a<b and b<c` (not `(a<b)<c`)? (CMP-*.)
9. **Rendering.** Does `str(x)` / f-string / `print` render as specified
   (`str(bool)` → `True`/`False`, float always with a decimal point)? Remember
   bare `print` float/bool format is a *separate* concern from `str()`.
10. **Faults & exceptions.** Do built-in faults your feature can raise
    (index-out-of-range, divide-by-zero, `None` access, bad parse) unwind as
    **catchable** exceptions (D-0007), with `finally`/`with` running? Not a
    process abort.
11. **Scoping & mutation.** Reference vs value semantics as specified (D-0023):
    does passing/assigning copy (scalars, tuples, `value struct`) or share
    (list/dict/struct)? Does a comprehension leak its loop variable? (It must
    not.)

## What to do with the answers

* **Matches Python / matches SPEC.md:** add at least one **conformance case**
  (`tests/conformance/`, see its README) for the edge you checked, and flip its
  `CLAIMS.md` row to `covered`. Edge-case conformance tests are required — this
  is how the gain is locked in.
* **Intentional divergence:** write the `D-00xx` decision, update `SPEC.md`, and
  pin it with a conformance case (expected = Zap's specified output) **and** a
  `# diff: diverge <reason>` case in `tests/diff/` so the difference from Python
  is documented, not hidden.
* **Unintended divergence you can't fix in this PR:** file it, add a `TODO` row
  to `CLAIMS.md` flagged `(impl gap …)`, and call it out in the PR description —
  do not pin the buggy behavior as a passing case.
