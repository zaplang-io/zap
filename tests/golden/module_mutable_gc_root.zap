# GC-root correctness (epic #661 point 7; D-0112, #662): a reference-typed
# mutable global is a permanent GC root. The runtime registers the ADDRESS of
# its data-segment slot (zap_gc_add_root), so the collector re-scans and, under
# the precise Immix collector, relocates through it on every collection — both
# the conservative Boehm collector (which scans the data segment anyway) and the
# precise collector keep the structure it holds live even as a large amount of
# short-lived garbage is allocated between mutations.
#
# Per project rule this asserts the structure's CONTENTS survive (length + sum),
# NEVER exact heap-byte counts (those are nondeterministic across collectors).
list[int64] kept = []

def churn():
    # Allocate and immediately discard a lot of garbage to pressure the GC.
    auto i = 0
    while i < 2000:
        auto junk = [i, i + 1, i + 2, i + 3]
        i += 1

def main():
    auto n = 0
    while n < 50:
        kept.append(n)
        churn()
        n += 1
    # If `kept` were not a root, a collection triggered inside churn() could
    # reclaim its backing store or elements; instead the contents are intact.
    print(len(kept))  # 50
    auto total = 0
    for x in kept:
        total += x
    print(total)  # 1225  (0 + 1 + ... + 49)
