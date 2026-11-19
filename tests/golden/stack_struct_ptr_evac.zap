# GC move-safety for a stack-promoted POINTER-BEARING struct (#1081). `h` is a
# non-escaping reference struct with a `str` field, so escape analysis places it
# in a STACK slot (no heap `StructNew`) and — because it still holds an outgoing
# heap pointer — registers an aggregate-root `(base, desc)` pair (the #586
# by-value-spill mechanism) rather than a direct root, so the collector traces
# its interior pointer field in place. `h` is then held live across several
# evacuating collections that relocate its `data` buffer out of the sparse blocks
# the churn leaves behind. The aggregate root MUST rewrite `h.data` in place on
# each move, so reading it back yields the exact original bytes; a missed or stale
# rewrite would print garbage or crash. The scalar `tag` field confirms the
# non-pointer part of the stack slot is undisturbed. Deterministic stdout, no
# heap-byte assertions — identical under the moving Immix collector and the
# non-moving Boehm oracle. (That `h` is actually stack-promoted here is pinned by
# the `escape_promotes_pointer_bearing_reference_struct` IR test; this golden
# exercises the resulting aggregate-root tracing end to end under `gc-immix`.)
struct Holder:
    str data
    int64 tag

def itos(int64 n) -> str:
    if n == 0:
        return "0"
    list[str] digits = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"]
    auto s = ""
    auto x = n
    while x > 0:
        s = digits[x % 10] + s
        x = x // 10
    return s

def main():
    # `data` is a fresh runtime concat (a movable heap ZapStr), reachable ONLY
    # through the stack-resident `h` — so if the aggregate root fails to trace it,
    # the buffer is reclaimed and the read below sees freed memory.
    auto h = Holder(data="payload-" + itos(42), tag=7)
    # Churn short-lived garbage and force collections so the block holding
    # h.data's buffer goes sparse and is selected for evacuation while `h` stays
    # reachable on the stack.
    for r in range(0, 6):
        for j in range(0, 20000):
            auto g = "trash-" + itos(j)
        gc.collect()
    print(h.data)
    print(h.tag)
