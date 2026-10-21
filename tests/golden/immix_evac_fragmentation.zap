# GC evacuation / defragmentation correctness (ticket #120). Allocate many heap
# strings but keep only a SPARSE subset alive (every 400th), so the blocks holding
# the survivors end up mostly dead once the rest are reclaimed — exactly the
# fragmented blocks the moving Immix collector selects as evacuation sources.
# Several rounds of garbage churn + forced collection then relocate the surviving
# string DATA buffers out of those sparse blocks into dense ones. Because the
# collector is precise, every reference to a moved buffer (each string's `data`
# field) is rewritten to its new address, and compiled code re-reads the relocated
# pointers after the safepoint. Printing the survivors therefore reproduces their
# exact bytes: a stale/un-updated reference would read freed or reused memory and
# print garbage (or crash). Deterministic stdout, no heap-byte assertions;
# identical under the moving Immix collector and non-moving Boehm.
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
    list[str] keep = []
    for i in range(0, 20000):
        auto s = "item-" + itos(i)
        if i % 400 == 0:
            keep.append(s)
    # Churn short-lived garbage and force collections so the sparse blocks holding
    # the survivors are evacuated while the survivors stay reachable via `keep`.
    for r in range(0, 6):
        for j in range(0, 20000):
            auto g = "trash-" + itos(j)
        gc.collect()
    for s in keep:
        print(s)
