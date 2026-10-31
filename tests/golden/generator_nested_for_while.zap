# Golden fixture / repro (I#453): a generator METHOD with an outer `while` loop
# and an inner `for` loop that yields — the exact control-flow of the AWS SDK
# paginator `items` method (walk pages in a `while`, yield each page's elements
# in an inner `for`). The inner iterator's state must survive the `yield`
# suspend/resume inside the outer loop. `n=3` builds rows [0,100], [1,101],
# [2,102]; summing every element gives 100+102+104 = 306.
struct Pages:
    int64 n

    def every(self):
        auto i = 0
        while i < self.n:
            list[int64] row = [i, i + 100]
            for x in row:
                yield x
            i = i + 1


def main():
    auto total = 0
    for v in Pages(n=3).every():
        total = total + v
    print(total)
