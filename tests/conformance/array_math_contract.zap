# claim: ARR-6
# SPEC §15 (#505): element-wise math intrinsics over arrays. `math.sqrt`/`exp`/
# `log`/`sin`/`cos`/`tan`/`fabs` map over a numeric array and return a new float
# array (shape preserved; an integer array promotes to f64). Scalar `math.<fn>`
# is unchanged.
def main():
    print("sqrt", sum(math.sqrt(array[f64]([4.0, 9.0, 16.0]))))
    print("sqrt_i", sum(math.sqrt(array[i64]([1, 4, 9]))))
    print("exp", math.exp(array[f64]([0.0]))[0])
    print("fabs", sum(math.fabs(array[f64]([-3.5, 2.5]))))
    print("nd", sum(math.sqrt(array[f64]([[4.0, 9.0], [16.0, 25.0]]))))
    print("scalar", int64(math.sqrt(2.0)))
