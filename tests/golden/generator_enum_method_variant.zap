# Golden fixture: a generator method whose *iteration depends on the enum
# discriminant* (I#449). `items` is a plain `while`/`yield` loop (the proven
# I#429 paginator shape), but both its bound (`self.size()`) and each yielded
# value (`self.at(i)`) come from sibling methods that `match self` — so the
# captured receiver's tag and payload steer how many values the generator
# produces and what they are, across every suspend/resume. `Count(n)` yields
# 0..n-1; `Pair(a, b)` yields its two payload fields. `Count(4)` sums to 6
# (0+1+2+3); `Pair(100, 200)` sums to 300.
enum Steps:
    Count(int64 n)
    Pair(int64 a, int64 b)

    def size(self) -> int64:
        match self:
            case Steps.Count(n):
                return n
            case Steps.Pair(a, b):
                return 2

    def at(self, int64 i) -> int64:
        match self:
            case Steps.Count(n):
                return i
            case Steps.Pair(a, b):
                if i == 0:
                    return a
                return b

    def items(self):
        auto i = 0
        while i < self.size():
            yield self.at(i)
            i = i + 1


def main():
    auto counted = Steps.Count(4)
    auto c_total = 0
    for x in counted.items():
        c_total = c_total + x
    print(c_total)
    auto paired = Steps.Pair(100, 200)
    auto p_total = 0
    for x in paired.items():
        p_total = p_total + x
    print(p_total)
