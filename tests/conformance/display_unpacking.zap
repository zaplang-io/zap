# claim: DSP-1
# SPEC §4 (#598): `*expr` splices an iterable's elements into a list/set/tuple
# display in place, and `**expr` merges another dict's entries with last-wins
# semantics. Output is reduced to scalars via len() and known-key access, so it
# does not depend on container iteration order.
def main():
    auto xs = [2, 3]
    auto ys = [1, *xs, 4]
    print(len(ys))
    print(ys[1])
    auto base = {"a": 1, "b": 2}
    auto m = {**base, "a": 9}
    print(m["a"])
    print(m["b"])
    print(len(m))
