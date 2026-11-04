# claim: WAL-1
# SPEC §5 (#600, epic #614 D8): the walrus `(name := expr)` evaluates its
# operand exactly once, binds `name` by inference in the lexically enclosing
# scope, and yields the value. Covered in all three positions — a `while`
# condition, an `if` condition, and a comprehension `if` filter — and the
# binding outlives the construct. Output is reduced to scalars.
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
