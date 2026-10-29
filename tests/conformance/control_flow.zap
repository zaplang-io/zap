# claim: CTRL-1, CTRL-2, CTRL-3
# SPEC §6: `if`/`elif`/`else` select exactly one branch (CTRL-1); `while` loops
# while its condition is truthy (CTRL-2); `for i in range(a, b)` iterates the
# half-open range `[a, b)` (CTRL-3).
def classify(int64 x) -> str:
    if x > 0:
        return "pos"
    elif x == 0:
        return "zero"
    else:
        return "neg"


def main():
    print(classify(5))
    print(classify(0))
    print(classify(-3))
    auto n = 3
    auto acc = 0
    while n > 0:
        acc = acc + n
        n = n - 1
    print(acc)
    auto s = 0
    for i in range(0, 5):
        s = s + i
    print(s)
