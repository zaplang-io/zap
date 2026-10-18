# Golden fixture: set[int64] — literal (with duplicates), len, add, membership
# (`in`), and for-in iteration. Iteration order is unspecified, so the test
# sums the elements (order-independent).
def main():
    auto s = {3, 1, 2, 2, 3}
    print(len(s))
    s.add(4)
    s.add(1)
    print(len(s))
    auto total = 0
    for x in s:
        total = total + x
    print(total)
    if 2 in s:
        print(1)
    else:
        print(0)
    if 9 in s:
        print(1)
    else:
        print(0)
