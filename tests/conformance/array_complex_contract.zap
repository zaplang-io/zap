# claim: ARR-20
# SPEC §15 (#938): the dense complex128 array dtype `array[complex]`. Each element
# is a (re, im) f64 pair; `a[i]` yields a complex scalar. Construction accepts
# complex and promoted real/int literals. Element-wise + - * / (array⊗array and
# array⊗scalar, real or complex), unary neg, `.conj()` (-> complex array) and
# `.abs()` (-> f64 magnitudes), whole-array sum, and real⊗complex promotion all
# operate element-wise. (complex is a value element / array dtype, not a dict/set
# key.)

def main():
    auto a = array[complex]([complex(3.0, 4.0), complex(0.0, -5.0)])
    auto b = array[complex]([complex(0.0, 1.0), complex(1.0, 0.0)])
    print("idx", str(a[0]), str(a[1]), len(a))  # (3+4j) (0-5j) 2
    print("mul", str((a * b)[0]))  # (-4+3j)
    print("abs", str(a.abs()[0]), str(a.abs()[1]))  # 5.0 5.0
    print("conj", str(a.conj()[0]))  # (3-4j)
    print("sum", str(sum(a)))  # (3-1j)

    auto r = array[complex]([2.0, 3.0])
    print("mix", str((r + a)[0]))  # (5+4j)
