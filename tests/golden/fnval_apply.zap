# First-class function values (I#163): a top-level function passed as an
# argument and invoked indirectly through a `fn(int64)->int64` parameter. Proves
# the function-value type parses, type-checks, and lowers, and that a bare
# function used as a value becomes a callable closure record.
def inc(int64 x) -> int64:
    return x + 1

def twice(int64 x) -> int64:
    return x * 2

def apply(fn(int64) -> int64 f, int64 x) -> int64:
    return f(x)

def main():
    print(apply(inc, 10))
    print(apply(twice, 10))
    # The same parameter accepts a lambda value.
    print(apply(lambda n: n - 3, 10))
