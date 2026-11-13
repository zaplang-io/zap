# claim: ARR-7
# SPEC §15 (#506): array true division `/` and non-commutative scalar-on-left.
# `/` is NumPy true division — integer operands promote to f64, float operands
# keep their width; a zero divisor yields IEEE inf/nan, not a raise (D-0169).
# `scalar - a` / `scalar / a` keep left-operand order (the commutative
# `s + a` / `s * a` reorder). Floor division `//` is not yet supported.
def main():
    print("div_i", sum(array[i64]([1, 2, 4]) / array[i64]([2, 2, 2])))
    print("div_s", sum(array[i64]([3, 6]) / 3))
    print("div_f32", sum(array[f32]([3.0, 6.0]) / 3))
    print("sdiv", sum(24 / array[i64]([2, 3, 4])))
    print("ssub", sum(10 - array[i64]([1, 2, 3])))
    auto zi = array[i64]([1]) / array[i64]([0])
    print("zero", str(zi[0]))
    auto zf = array[f64]([0.0]) / array[f64]([0.0])
    print("nan", str(zf[0]))
