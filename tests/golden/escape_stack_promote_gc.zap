# #839 (discussion #849): stack-promoted non-escaping allocations stay correct
# across garbage collection. A leaf (all-scalar) reference struct built per
# iteration and used only locally is stack-allocated (StackStruct); a transient
# f"k{i}" dict-lookup key is stack-allocated (StrBuild stack buffer). Both must
# produce exact results across forced collections with heavy churn — the stack
# slots are never relocated and hold no heap pointers, so no root is missed and
# no value is read stale. Asserts the derived SUM only (never heap bytes), and
# the same program must pass under both the Boehm and precise Immix collectors.

struct Point:
    int64 x = 0
    int64 y = 0

def churn():
    # Sparse, mostly-dead heap allocation so the moving collector has garbage to
    # reclaim / blocks to evacuate between the stack-promoted work below.
    for i in range(0, 2000):
        auto junk = [str(i), str(i + 1)]

def main():
    # Build 100 string keys that ARE retained by the dict (these escape -> heap).
    dict[str, int64] d = {}
    int64 i = 0
    while i < 100:
        d[f"k{i}"] = i
        i = i + 1
    gc.collect()
    churn()
    gc.collect()

    # The hot loop: each iteration builds a non-escaping leaf `Point` (stack) and
    # a non-escaping transient `f"k{i}"` lookup key (stack). term = x + y + d[k]
    # = i + (i+1) + i = 3i + 1; sum over i in 0..99 = 3*4950 + 100 = 14950.
    int64 s = 0
    i = 0
    while i < 100:
        auto p = Point(x=i, y=i + 1)
        s = s + p.x + p.y + d[f"k{i}"]
        i = i + 1
    print(s)
