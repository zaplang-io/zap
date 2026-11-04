# Golden fixture (compile-fail): `*args` in a `wrap` inner body is a
# compile-time forwarding template, not a runtime value (D-0036 / D-0108; #653).
# Reading it (`print(args)`) is an unsupported body shape and reports ZAP5052.

decorator bad:
    def wrap(call):
        def inner(*args):
            print(args)
            return call(*args)
        return inner

@bad
def f(int64 x) -> int64:
    return x

def main():
    print(f(1))
