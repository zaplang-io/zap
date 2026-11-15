# Golden fixture: the complex128 array dtype `array[complex]` (#938). Elements are
# 16-byte (re, im) f64 pairs; every op is a runtime helper (a complex element is
# not a scalar SIMD lane). Construction (from complex and from promoted real
# literals), indexing (-> a complex scalar), element-wise + - * / (array⊗array and
# array⊗scalar), unary neg, conj/abs methods, sum, and real⊗complex mixing. Values
# are chosen clean (integer re/im, magnitudes 5) so the (%g±%gj) / float rendering
# is exact.

def main():
    auto a = array[complex]([complex(3.0, 4.0), complex(0.0, -5.0)])
    auto b = array[complex]([complex(0.0, 1.0), complex(1.0, 0.0)])
    print("get", str(a[0]), str(a[1]), len(a), a.ndim)  # (3+4j) (0-5j) 2 1

    # construction from promoted real / integer literals (imag 0).
    auto r = array[complex]([5.0, 6.0])
    auto ic = array[complex]([1, 2])
    print("real", str(r[0]), str(ic[1]))  # (5+0j) (2+0j)

    # element-wise array ⊗ array.
    print("add", str((a + b)[0]), str((a + b)[1]))  # (3+5j) (1-5j)
    print("sub", str((a - b)[0]))  # (3+3j)
    print("mul", str((a * b)[0]), str((a * b)[1]))  # (-4+3j) (0-5j)
    print("div", str((a / b)[0]))  # (4-3j)

    # scalar broadcast: real scalar and complex scalar.
    print("scal", str((a * 2.0)[0]), str((a * complex(0.0, 1.0))[0]))  # (6+8j) (-4+3j)

    # unary neg, conj, abs, sum.
    print("neg", str((-a)[0]))  # (-3-4j)
    print("conj", str(a.conj()[0]))  # (3-4j)
    auto ab = a.abs()
    print("abs", str(ab[0]), str(ab[1]))  # 5.0 5.0
    print("sum", str(sum(a)))  # (3-1j)

    # real array ⊗ complex array promotes to complex.
    print("mix", str((r + a)[0]))  # (8+4j)
