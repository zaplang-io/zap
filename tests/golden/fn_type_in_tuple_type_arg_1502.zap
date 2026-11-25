# Regression for #1502 (follow-up to #1488): a function type nested inside a
# tuple type argument, e.g. `run[(fn(A) -> B, C)]`. Before the fix the
# parenthesized tuple there was parsed as a value expression and a function type
# is not a value expression, so it failed to parse. The parenthesized-tuple
# parser now admits a `fn(` function type as a tuple element.
#
# `zap fmt` canonicalizes a tuple type argument to its paren-free spelling (as
# it does for every tuple type argument), so this fixture is written in that
# canonical form — `run[fn(A) -> B, C]` — which is what the formatter emits and
# what `zap fmt --check` requires. It exercises the end-to-end value of a
# function type carried inside a tuple type argument: a free function, a method,
# a lambda, a second function-type element, and a void-return element.


struct Box:
    int64 unused

    def run[T](self, T fallback) -> T:
        return fallback


def run[T](T fallback) -> T:
    return fallback


def inc(int64 x) -> int64:
    return x + 1


def dbl(int64 x) -> int64:
    return x * 2


def shout(int64 n):
    print(n)


def main():
    # Free function: a function type paired with a scalar in a tuple type arg.
    auto pair = run[fn(int64) -> int64, int64]((inc, 5))
    auto f, n = pair
    print(f(n))  # 6

    # Method receiver, with a lambda value.
    auto box = Box(0)
    auto mpair = box.run[fn(int64) -> int64, int64]((lambda x: x * 10, 4))
    auto mf, mn = mpair
    print(mf(mn))  # 40

    # Two function-type elements in one tuple type argument.
    auto fns = run[fn(int64) -> int64, fn(int64) -> int64]((inc, dbl))
    auto a, b = fns
    print(a(10), b(10))  # 11 20

    # A void-return function type as a tuple element.
    auto vpair = run[fn(int64), int64]((shout, 7))
    auto s, k = vpair
    s(k)  # 7
