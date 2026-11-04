# #663 (epic #661): reassigning a mutable module-level pointer global across a
# GC cycle is safe. The reassignment stores a fresh structure through the
# permanent root slot (the write barrier, D-0112), and the collector marks and
# — under precise Immix — relocates through that slot on every collection. So
# after forced collections with heavy garbage pressure the global holds exactly
# the most-recently-assigned structure: never a stale pointer to a reclaimed or
# relocated old value, never a value collected out from under the slot.
#
# Asserts the structure's CONTENTS and element identity only (length, elements,
# a derived width sum) — NEVER heap-byte counts, which are nondeterministic
# across the Boehm and precise Immix collectors. The same program must pass
# under both legs.
list[str] items = ["seed"]

def churn():
    # Sparse, mostly-dead allocation so the moving collector has fragmented
    # blocks to evacuate between and after the reassignments.
    for i in range(0, 4000):
        auto junk = [str(i), str(i + 1), str(i + 2)]

def main():
    # First reassignment: the seed list becomes garbage; `items` must hold the
    # newly assigned list after the collections, with its string elements intact.
    items = ["a", "bb", "ccc"]
    churn()
    gc.collect()
    churn()
    gc.collect()
    print(len(items))  # 3
    auto widths = 0
    for s in items:
        widths += len(s)
    print(widths)  # 1 + 2 + 3 = 6
    print(items[0] + items[2])  # a + ccc = accc

    # Second reassignment across more cycles: a longer list whose relocated
    # string elements must still read back correctly.
    items = ["x", "yy", "zzz", "wwww", "vvvvv"]
    gc.collect()
    churn()
    gc.collect()
    print(len(items))  # 5
    print(items[0])  # x
    print(items[4])  # vvvvv
    auto total = 0
    for s in items:
        total += len(s)
    print(total)  # 1 + 2 + 3 + 4 + 5 = 15
