# Golden fixture (compile-fail): a `wrap(call)` hook must return the nested
# callable it defines (D-0036; #653). Here `wrap` ends with `return 0`, not
# `return inner`, so expansion reports ZAP5054 and compilation fails.

decorator bad:
    def wrap(call):
        def inner(*args):
            return call(*args)
        return 0

@bad
def f(int64 x) -> int64:
    return x

def main():
    print(f(1))
