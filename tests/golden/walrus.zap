# Golden fixture: the walrus assignment-expression `(name := expr)` (#600, epic
# #614 D8). It binds `name` by inference in the lexically enclosing scope and
# evaluates to the operand's value. This exercises all three positions — a
# `while` condition, an `if` condition, and a comprehension `if` filter — and
# shows that each binding outlives its construct and that the operand runs
# exactly once per evaluation (the four "eval" lines count the `while`
# condition's evaluations: three that iterate plus the one that stops it).
def announce(int64 k) -> int64:
    print("eval")
    return k


def main():
    # `while` condition: `m` is bound once per iteration and is visible after.
    int64 k = 0
    while (m := announce(k)) < 3:
        k = k + 1
    print(m)

    # `if` condition: `n` is usable in the branch body and after the `if`.
    list[int64] xs = [10, 20, 30]
    if (n := len(xs)) >= 3:
        print(n)
    print(n + 1)

    # Comprehension `if` filter: `y` is bound per element and, as in Python,
    # leaks to the enclosing scope — so its last value is visible afterwards.
    list[int64] evens = [y for v in xs if (y := v // 10) % 2 == 0]
    print(len(evens))
    print(y)
