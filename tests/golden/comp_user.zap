# Golden fixture: a comprehension over a user-defined iterable (iteration
# protocol, D-0048). `[x*x for x in Range(1,5) if x != 3]` = [1, 4, 16]; summing
# the result list gives 21.
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
    auto squares = [x * x for x in Range(lo=1, hi=5) if x != 3]
    auto total = 0
    for s in squares:
        total = total + s
    print(total)
