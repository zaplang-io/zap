# Golden fixture: the scalar `int` type (I#581), the Python-facing name for the
# 64-bit signed integer (`int64`/`i64`). It works as a binding, a function
# parameter and return, in arithmetic and comparison, and as a `list[int]`
# element — which reuses the existing i64 collection word, so no new runtime.
def add(int a, int b) -> int:
    return a + b

def main():
    list[int] xs = [1, 2, 3]
    print(len(xs))  # 3
    int total = 0
    for x in xs:
        total = total + x
    print(total)  # 6
    print(add(4, 5))  # 9
    int a = 7
    print(a // 2)  # 3  (floor division stays integer)
    print(a > 3)  # True
    print(a == 7)  # True
