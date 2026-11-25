# Regression for #1488: a function type used as an explicit generic type
# argument, `run[fn(A) -> B](...)`. Before the fix the parser rejected a
# function type in a `[...]` subscript (`ZAP2001: expected ']'`) because a
# subscript element was only ever a value expression; a bare function name or
# lambda also could not pin a generic type parameter by inference, so a function
# value could reach a generic `T` only through an intermediate `fn`-typed
# binding. The parser now admits a `func_type` in type-argument position and
# carries it as a `TypeArg` that mono's `expr_to_type` unwraps. Covers the
# free-function and method forms, a lambda and a named-function argument, a
# multi-parameter function type, and a void-return function type.


struct Box:
    int64 unused

    def run[T](self, T fallback) -> T:
        return fallback


def run[T](T fallback) -> T:
    return fallback


def inc(int64 x) -> int64:
    return x + 1


def add(int64 a, int64 b) -> int64:
    return a + b


def shout(int64 n):
    print(n)


def main():
    # Free function, lambda argument.
    auto f = run[fn(int64) -> int64](lambda x: x + 1)
    print(f(41))  # 42

    # Free function, named-function argument (inference alone could not pin T).
    auto g = run[fn(int64) -> int64](inc)
    print(g(9))  # 10

    # Generic method with an explicit function-type argument.
    auto box = Box(0)
    auto h = box.run[fn(int64) -> int64](lambda x: x * 2)
    print(h(21))  # 42

    # Multi-parameter function type.
    auto two = run[fn(int64, int64) -> int64](add)
    print(two(2, 3))  # 5

    # Void-return function type.
    auto s = run[fn(int64)](shout)
    s(7)  # 7
