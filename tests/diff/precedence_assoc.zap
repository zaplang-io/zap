# diff: match
# Operator precedence and associativity match CPython (epic #374, walled garden).
# `**` is right-associative and binds tighter than unary minus; `*`/`//`/`%`
# bind tighter than `+`/`-`; shifts looser than `+`; `&` tighter than `|`;
# comparisons looser than the bitwise/arithmetic operators; `not` looser than a
# comparison. CPython is the oracle.
def main():
    print(2 ** 3 ** 2)
    print(-2 ** 2)
    print(1 + 2 * 3)
    print(10 - 3 - 2)
    print(8 // 3 * 2)
    print(2 * 3 % 4)
    print(1 << 2 + 3)
    print(1 | 2 & 3)
    print(7 & 3 | 8)
    print(5 & 3 == 1)
    print(3 > 2 > 1)
    print(not 1 == 1)
