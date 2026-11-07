# diff: match
# Augmented assignment matches CPython across the operator set (epic #374, walled
# garden). Integer `+= -= *= //= %= <<= >>= &= |= ^= **=` and float `/=` all give
# the same result as the expanded `x = x <op> y`. (Int `/=` would change the type
# to float, so true division is exercised on a float target.)
def main():
    int64 a = 10
    a += 3
    print(a)
    a -= 5
    print(a)
    a *= 2
    print(a)
    a //= 3
    print(a)
    a %= 4
    print(a)
    a <<= 4
    print(a)
    a >>= 2
    print(a)
    a &= 6
    print(a)
    a |= 1
    print(a)
    a ^= 3
    print(a)
    a **= 2
    print(a)
    f64 x = 7.0
    x /= 2.0
    print(x)
