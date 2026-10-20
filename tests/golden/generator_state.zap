# Golden fixture: a generator that carries state across multiple `yield`s
# (I#67). `fib(10)` yields the first ten Fibonacci numbers from live locals
# `a`/`b` that persist across each suspend/resume; summing them gives 88.
def fib(int64 k):
    auto a = 0
    auto b = 1
    auto n = 0
    while n < k:
        yield a
        auto t = a + b
        a = b
        b = t
        n = n + 1

def main():
    auto s = 0
    for x in fib(10):
        s = s + x
    print(s)
