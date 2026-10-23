# Golden fixture: mixed-dtype array promotion (ticket #49).
# Element-wise ops on arrays of different dtypes promote to the higher dtype:
#   bool < uint8 < i64 < f64.
# The SIMD kernel loads each operand at its own storage type, widens it to the
# result dtype, and computes in that dtype.
def main():
    auto ai = array[i64]([1, 2, 3, 4, 5])
    auto af = array[f64]([0.5, 0.5, 0.5, 0.5, 0.5])
    print(sum(ai + af))  # i64 + f64 -> f64: 1.5+2.5+3.5+4.5+5.5 = 17.5

    auto au = array[uint8]([10, 20, 30, 40, 50])
    print(sum(au + ai))  # uint8 + i64 -> i64: 11+22+33+44+55 = 165

    auto m = array[bool]([1, 0, 1, 0, 1])
    print(sum(m + af))  # bool + f64 -> f64: 1.5+0.5+1.5+0.5+1.5 = 5.5

    auto r = ai + af  # f64 result array
    print(r[2])  # 3.5
