# Golden fixture: bool arrays (ticket #49) — the mask dtype.
# Stored as 0/1 bytes (any nonzero normalizes to 1 on construction). Arithmetic
# promotes bool -> uint8 (True + True == 2, which a 1-bit result could not
# hold), so `m + n` counts and `m * n` is a mask intersection. sum() counts the
# set bits.
def main():
    auto m = array[bool]([1, 0, 1, 1, 0, 1])   # 4 set
    auto n = array[bool]([1, 1, 0, 1, 0, 0])   # 3 set
    print(sum(m))        # 4
    print(sum(n))        # 3
    auto u = m + n       # bool + bool -> uint8: 2,1,1,2,0,1
    print(sum(u))        # 7
    print(u[0])          # 2 (uint8 element, read as i64)
    auto v = m * n       # mask AND -> uint8: 1,0,0,1,0,0
    print(sum(v))        # 2
    print(len(m))        # 6
