# diff: match
# Augmented assignment to a subscript target matches CPython across the operator
# set (#374, #750). A list element `xs[i] OP= v` reads, combines, and writes back
# the one element for `+= -= *= //= %= <<= >>= &= |= ^=`; a dict value
# `d[k] OP= v` does the same. The element is read with the same semantics as a
# plain `xs[i]` read.
def main():
    list[int64] xs = [10]
    xs[0] += 5
    print(xs[0])
    xs[0] -= 3
    print(xs[0])
    xs[0] *= 2
    print(xs[0])
    xs[0] //= 5
    print(xs[0])
    xs[0] %= 3
    print(xs[0])
    xs[0] <<= 4
    print(xs[0])
    xs[0] >>= 2
    print(xs[0])
    xs[0] &= 6
    print(xs[0])
    xs[0] |= 1
    print(xs[0])
    xs[0] ^= 3
    print(xs[0])
    dict[str, int64] d = {"a": 100}
    d["a"] += 1
    print(d["a"])
    d["a"] *= 2
    print(d["a"])
