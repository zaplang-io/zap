# diff: match
# A closure captures its defining environment and outlives the frame that made
# it (I#163). Two independently-made closures each keep their own `n`, exactly
# as in CPython. (Closures are called through direct variable names; calling a
# closure fetched from a list element — `fns[0]()` — is not yet supported by
# codegen, so it is left out of this must-match case.)
def make_adder(int64 n) -> fn(int64) -> int64:
    return lambda x: x + n


def main():
    auto add5 = make_adder(5)
    auto add100 = make_adder(100)
    print(add5(10))
    print(add100(10))
    print(add5(1))
    print(add100(2))
