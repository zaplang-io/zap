# diff: match
# `*`/`**` unpacking in displays (#598) and multiple `for`/`if` clauses in
# comprehensions (#599) share CPython's semantics over this subset, so stdout
# must agree. `*expr`/`**expr` in list/tuple/dict displays and multi-clause
# comprehensions are valid Python verbatim, so the translator passes them
# through unchanged. Results are reduced to scalars via len() and indexing (and
# dict access by known key) so the check never depends on container repr or
# iteration order; comprehension and splice order is left-to-right in both
# languages. CPython is the oracle.
def main():
    auto xs = [2, 3]
    auto ys = [1, *xs, 4, *xs]
    print(len(ys))
    print(ys[0])
    print(ys[5])

    auto tup = (10, 20)
    auto t = (1, *tup, 2)
    print(t[0])
    print(t[1])
    print(t[3])

    auto base = {"a": 1, "b": 2}
    auto m = {"a": 100, **base, "c": 3}
    print(len(m))
    print(m["a"])
    print(m["b"])
    print(m["c"])

    auto prod = [x * y for x in [1, 2, 3] for y in [1, 2, 3] if x != y]
    print(len(prod))
    print(prod[0])
    print(prod[5])

    auto sums = [x + y for x in range(3) if x > 0 for y in range(3) if y > 0]
    print(len(sums))
    print(sums[0])
    print(sums[3])
