# Golden fixture: the scalar numeric tower (ticket #19 / T9).
#
# Exercises int32 / float32 storage, loading, printing, and passing/returning
# across a call; explicit numeric conversions (D-0020: widening is implicit,
# narrowing and int<->float need an explicit cast); the `**` power operator for
# ints and floats; and Python floor-division `//` (D-0024), including negative
# operands to prove it rounds toward negative infinity, not toward zero.

def scale(int32 n) -> int32:
    # Arithmetic widens to int64 (D-0019), so an int32 result needs an explicit
    # narrowing conversion back.
    return int32(n * 2)

def main():
    int32 a = 7
    int32 b = scale(a)
    print(a)                 # 7
    print(b)                 # 14

    float32 f = 1.5
    print(f)                 # 1.500000
    float64 g = f            # implicit float32 -> float64 widening
    print(g + 0.5)           # 2.000000

    int64 big = 300
    int32 small = int32(big) # explicit narrowing int64 -> int32
    print(small)             # 300
    print(float64(a))        # 7.000000  (int -> float)
    print(int64(2.9))        # 2         (float -> int, truncates toward zero)

    print(2 ** 10)           # 1024      (integer power)
    print(2.0 ** 0.5)        # 1.414214  (float power, sqrt 2)

    print(7 // 2)            # 3
    print(-7 // 2)           # -4        (toward -inf; C truncation gives -3)
    print(7 // -2)           # -4
    print(-7.0 // 2.0)       # -4.000000 (float floor division)
