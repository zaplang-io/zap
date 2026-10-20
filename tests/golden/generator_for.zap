# Golden fixture: a generator (`yield`) consumed by a `for` loop (I#67,
# D-0026/D-0048). `count(5)` is a generator — calling it returns an Iterator
# whose state machine yields 0,1,2,3,4 — and drops straight into `for` via the
# #53 protocol. Summing 0..4 gives 10.
def count(int64 n):
    auto i = 0
    while i < n:
        yield i
        i = i + 1

def main():
    auto total = 0
    for x in count(5):
        total = total + x
    print(total)
