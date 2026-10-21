# A closure capturing a local (I#163): `make_adder` returns a `lambda` that
# captures its parameter `n`. The returned closure is invoked AFTER `make_adder`
# has returned, so the captured environment cannot live in the returning frame —
# it is a GC-managed heap record. Two independent closures prove each captures
# its own `n`.
def make_adder(int64 n) -> fn(int64)->int64:
    return lambda x: x + n

def main():
    auto add5 = make_adder(5)
    auto add100 = make_adder(100)
    print(add5(10))
    print(add100(10))
    print(add5(1))
