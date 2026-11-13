# Golden fixture: array true division `/` and non-commutative scalar-on-left
# (#506, D-0169). `/` is NumPy true division: integer arrays promote to f64 and
# float arrays keep their width; a zero divisor yields IEEE inf/nan, never a
# raise. The non-commutative `scalar - a` / `scalar / a` keep left-operand
# order (the commutative `s + a` / `s * a` reorder to `a + s` / `a * s`).
def main():
    # array / array: integer true division -> f64.
    auto a = array[i64]([6, 9])
    auto b = array[i64]([2, 3])
    print(sum(a / b))  # 3.0 + 3.0 -> 6.0

    # array / scalar: int/int -> f64 (half-values survive).
    print(sum(array[i64]([1, 2]) / 2))  # 0.5 + 1.0 -> 1.5

    # f32 array / int scalar keeps f32 width (weak scalar).
    print(sum(array[f32]([1.0, 2.0]) / 2))  # 1.5

    # scalar / array (scalar-on-left, true division).
    print(sum(12 / array[i64]([2, 3, 4])))  # 6+4+3 -> 13.0

    # scalar - array (scalar-on-left): stays i64.
    print(sum(10 - array[i64]([1, 2, 3])))  # 9+8+7 -> 24

    # array - scalar (scalar on the right): stays i64.
    print(sum(array[i64]([5, 6]) - 1))  # 4+5 -> 9

    # commutative scalar-on-left reorders (value unaffected).
    print(sum(2 + array[i64]([1, 2, 3])))  # 3+4+5 -> 12

    # broadcasting under `/`: a row vector stretched across the rows.
    auto m = array[i64]([[2, 4], [6, 8]])
    print(sum(m / array[i64]([2, 4])))  # [1,1],[3,2] -> 7.0

    # Divide by zero yields IEEE inf/nan, never a raise (D-0169).
    auto zi = array[i64]([1]) / array[i64]([0])
    print(str(zi[0]))  # 1.0 / 0.0 -> inf
    auto zf = array[f64]([0.0]) / array[f64]([0.0])
    print(str(zf[0]))  # 0.0 / 0.0 -> nan
    auto zn = array[f64]([-1.0]) / 0.0
    print(str(zn[0]))  # -1.0 / 0.0 -> -inf

    # No ZeroDivisionError escapes, even inside a try (unlike scalar `/`).
    try:
        auto t = array[i64]([5]) / array[i64]([0])
        print(str(t[0]))  # inf
    except ZeroDivisionError:
        print("raised")
