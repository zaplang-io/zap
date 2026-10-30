# Golden fixture: a generator method whose *control flow depends on the enum
# discriminant* (I#449). `items` matches `self` and yields a different number of
# values per variant — proving the captured receiver carries both its tag and
# its payload across the suspend/resume boundary. `Count(n)` yields 0..n-1 (a
# loop whose `yield` sits inside the match arm); `Pair(a, b)` yields its two
# payload fields. `Count(4)` sums to 6 (0+1+2+3); `Pair(100, 200)` sums to 300.
enum Steps:
    Count(int64 n)
    Pair(int64 a, int64 b)

    def items(self):
        match self:
            case Steps.Count(n):
                auto i = 0
                while i < n:
                    yield i
                    i = i + 1
            case Steps.Pair(a, b):
                yield a
                yield b


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
