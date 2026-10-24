# A small Zap sample used by the TextMate grammar tokenizer test.

import io


@memoize
def fib(int64 n) -> int64:
    """Return the nth Fibonacci number.

    A naive, exponential recursive definition — for the grammar test only.
    """
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


struct Point:
    int64 x
    int64 y

    def sum(self) -> int64:
        return self.x + self.y


def main() -> int64:
    const auto count = 42
    auto ratio = 3.14
    auto name = "world"
    auto flag = True
    auto nothing = None
    if flag and count > 0:
        io.println(f"hello {name}")
    return 0
