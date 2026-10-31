# Golden fixture / repro (I#453): a `for` loop INSIDE a generator body, yielding
# each element. Every other generator golden drives the generator from a `for`
# loop in `main` but uses only a `while` loop *inside* the generator; a `for`
# loop in the generator body (which desugars to its own iterator + `__next__`
# calls, whose state must survive the `yield` suspend/resume) is untested. This
# is the control-flow shape behind the AWS SDK paginator `items` method
# (`for it in page_items: yield it`). Sums 10+20+30 = 60.
def emit():
    list[int64] xs = [10, 20, 30]
    for x in xs:
        yield x


def main():
    auto total = 0
    for v in emit():
        total = total + v
    print(total)
