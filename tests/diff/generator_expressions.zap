# diff: match
# Bare generator expressions `(x for x in xs)` (#596) share CPython's semantics
# over this subset — a genexpr is a lazy iterator producing the same elements in
# the same order — so stdout must agree. A genexpr is valid Python verbatim, so
# the translator passes it through unchanged. Results are reduced to scalars via
# sum() and a running total so the check never depends on container repr or
# iteration order. CPython is the oracle.
def main():
    # sum over a genexpr as the sole call argument: 0+1+4+9+16 = 30
    auto total = sum((x * x for x in range(5)))
    print(total)

    # a genexpr bound to a name, consumed once by a for-loop; y in {30,40,50}
    auto xs = [1, 2, 3, 4, 5]
    auto g = (y * 10 for y in xs if y > 2)
    auto acc = 0
    for z in g:
        acc = acc + z
    print(acc)

    # multi-clause genexpr: ordered pairs x != y over 0..2
    # (0,1),(0,2),(1,0),(1,2),(2,0),(2,1) -> sum of x+y = 1+2+1+3+2+3 = 12
    auto s2 = sum((x + y for x in range(3) for y in range(3) if x != y))
    print(s2)
    # NOTE: a genexpr nested inside another genexpr (one generator driving a
    # second across its own `yield`) is the "nested generators" case I#67
    # explicitly deferred, so it is intentionally not exercised here.
