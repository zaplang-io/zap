# Golden fixture (#840): the SoA / tag table under a set workload — large int and
# string sets with duplicate adds (dedup), resize across many power-of-two
# growths, membership, and the never-present case. A tag collision between two
# distinct members must fall through to the full compare (never dedup two
# unequal members, never miss an equal one). Order-independent (len + sums).
def main():
    set[int64] s = set[int64]()
    auto n = 3000

    # Add 0..2999, then add every element a second time (dedup path on the chain).
    for i in range(0, n):
        s.add(i)
    for i in range(0, n):
        s.add(i)
    print(len(s))  # 3000  (duplicates ignored)

    int64 tot = 0
    for x in s:
        tot = tot + x
    print(tot)  # 0+..+2999 = 4498500

    int64 present = 0
    for i in range(0, n):
        if i in s:
            present = present + 1
    print(present)  # 3000
    print(n in s)  # False

    # String members (one ZapStr object per distinct key; byte-equal re-adds dedup).
    set[str] w = set[str]()
    for i in range(0, 1000):
        w.add(f"e{i}")
    for i in range(0, 1000):
        w.add("e" + str(i))  # same bytes as f"e{i}" -> no new member
    print(len(w))  # 1000
    print("e500" in w)  # True
    print("e9999" in w)  # False
    int64 wlen = 0
    for e in w:
        wlen = wlen + len(e)
    print(wlen)  # sum len("e"+str(i)) for i in 0..999
