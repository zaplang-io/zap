# Golden fixture (#840): heavy linear-probe collisions. Keys are multiples of 64,
# so after the power-of-two mask they cluster into a handful of home slots and
# form long probe chains — the case the 1-byte tag scan and the full-key compare
# must both get right. A tag collision on such a chain must still reject a
# non-equal key (fall through to the key compare), and a delete must tombstone
# (not break) the chain so later keys on it stay reachable. Order-independent.
def main():
    dict[int64, int64] d = {}
    auto n = 2000

    # Insert 64*i -> i for i in 0..1999: all share the low 6 bits, maximal
    # clustering / probe-chain length at every table size.
    for i in range(0, n):
        d[i * 64] = i
    print(len(d))  # 2000
    int64 s1 = 0
    for i in range(0, n):
        s1 = s1 + d[i * 64]
    print(s1)  # 0+..+1999 = 1999000

    # Delete the even-i keys (1000 tombstones scattered along the chains).
    int64 removed = 0
    for i in range(0, n):
        if i % 2 == 0:
            del d[i * 64]
            removed = removed + 1
    print(removed)  # 1000
    print(len(d))  # 1000

    # The odd-i keys must all still be reachable past the tombstones.
    int64 present = 0
    int64 s2 = 0
    for i in range(0, n):
        if i * 64 in d:
            present = present + 1
            s2 = s2 + d[i * 64]
    print(present)  # 1000
    print(s2)  # sum of odd i in 0..1999 = 1000*1000 = 1000000

    # A deleted key reads back as the default; a never-present key too.
    print(d.get(0, -1))  # -1  (i=0 was even -> deleted)
    print(d.get(1, -1))  # -1  (1 is not a multiple of 64)
    print(63 * 64 in d)  # True  (i=63 odd -> present)
