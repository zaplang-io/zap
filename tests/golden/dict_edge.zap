# Golden fixture (#840): empty / singleton / emptied-by-delete edges of the SoA
# table. An empty table (cap == 0, NULL buffer) must answer get/contains/len and
# the enumeration snapshots without touching the (absent) buffer; a lone key must
# round-trip; deleting the last key must leave the table correctly empty and a
# later insert must reuse it. Order-independent.
def main():
    # Empty dict: no buffer allocated yet.
    dict[int64, int64] d = {}
    print(len(d))  # 0
    print(5 in d)  # False
    print(d.get(5, -1))  # -1
    print(len(d.keys()))  # 0

    # Singleton.
    d[42] = 100
    print(len(d))  # 1
    print(d[42])  # 100
    print(42 in d)  # True
    print(43 in d)  # False

    # Delete the only key -> empty again (by tombstone, cap unchanged).
    del d[42]
    print(len(d))  # 0
    print(42 in d)  # False
    print(d.get(42, -9))  # -9

    # Reinsert after emptying: reuses the tombstoned slot.
    d[42] = 7
    print(len(d))  # 1
    print(d[42])  # 7

    # Empty set edges.
    set[int64] e = set[int64]()
    print(len(e))  # 0
    print(1 in e)  # False
    int64 cnt = 0
    for x in e:  # iterating an empty set runs the body zero times
        cnt = cnt + 1
    print(cnt)  # 0
