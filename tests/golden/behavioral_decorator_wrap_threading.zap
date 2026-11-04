# Golden fixture: a `wrap(call)` decorator threads multiple arguments through to
# the original and transforms the captured return value (D-0036; #653).
#
# `@doubled` captures the original's result with `auto r = call(*args)` and
# returns `r + r`. Because `inner` provably returns on every path, NO implicit
# fall-through guard is generated. `call(*args)` forwards all three typed
# parameters of `combine` unchanged.

decorator doubled:
    def wrap(call):
        def inner(*args):
            auto r = call(*args)
            return r + r
        return inner

@doubled
def combine(int64 a, int64 b, int64 c) -> int64:
    print("combine", a, b, c)
    return a + b + c

def main():
    print("result", combine(1, 2, 3))
