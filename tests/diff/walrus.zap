# diff: match
# The walrus assignment-expression `(name := expr)` (#600) shares CPython's PEP
# 572 semantics over this subset: it binds the name in the enclosing scope,
# evaluates to the operand's value, and runs the operand exactly once per
# evaluation. `(name := expr)` is valid Python verbatim, so the translator
# passes it through unchanged and CPython is the oracle. A `while` condition, an
# `if` condition, and a comprehension `if` filter are each covered; all results
# are reduced to scalars (counts and bound-name values) so the check never
# depends on container repr or iteration order.
def announce(int64 k) -> int64:
    print("eval")
    return k


def main():
    int64 k = 0
    while (m := announce(k)) < 3:
        k = k + 1
    print(m)

    auto xs = [10, 20, 30]
    if (n := len(xs)) >= 3:
        print(n)
    print(n + 1)

    auto evens = [y for v in xs if (y := v // 10) % 2 == 0]
    print(len(evens))
    print(y)
