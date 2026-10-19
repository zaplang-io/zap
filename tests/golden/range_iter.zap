# Golden fixture: a user-defined iterable driven by `for` (iteration protocol,
# D-0048). `Range.__iter__` yields a fresh comma-ok iterator each time, so the
# same `Range` can be iterated twice; each loop sums 0..4 = 10, total 20.
struct Range:
    int64 lo
    int64 hi
    def __iter__(self) -> RangeIter:
        return RangeIter(cur=self.lo, hi=self.hi)

struct RangeIter:
    int64 cur
    int64 hi
    def __iter__(self) -> RangeIter:
        return self
    def __next__(self) -> (int64, bool):
        if self.cur >= self.hi:
            return (0, False)
        auto v = self.cur
        self.cur = self.cur + 1
        return (v, True)

def main():
    auto r = Range(lo=0, hi=5)
    auto a = 0
    for x in r:
        a = a + x
    auto b = 0
    for x in r:
        b = b + x
    print(a + b)
