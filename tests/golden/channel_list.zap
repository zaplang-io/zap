# Golden fixture: a channel of a nested collection (#1100 / #625).
#
# A `channel[list[int]]` element is a reference, but the received value must
# recover its *collection* type, not default a bare pointer to `str`: `len()`
# and indexing on the received list must use the list entry points. Uses a
# buffered channel inline so the test turns purely on element read-back.
def main():
    auto c = channel[list[int]](1)
    auto xs = [10, 20, 30]
    c <- xs
    auto ys = <-c
    print(len(ys))  # 3
    print(ys[0])  # 10
    print(ys[2])  # 30
