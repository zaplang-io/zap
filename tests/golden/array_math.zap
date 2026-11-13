# Golden fixture: element-wise math over arrays (#505, D-0168). `math.<fn>(a)`
# maps the function over the array, yielding a new float array (f32 stays f32,
# everything else f64). Values are chosen to have exact results.
def main():
    # sqrt of perfect squares.
    auto s = math.sqrt(array[f64]([4.0, 9.0, 16.0]))
    print("sqrt", s[0], s[1], s[2], sum(s))  # 2.0 3.0 4.0 9.0
    # an integer array maps to an f64 result.
    print("sqrt_i", sum(math.sqrt(array[i64]([1, 4, 9]))))  # 6.0
    # exp(0)=1, log(1)=0.
    print("exp", math.exp(array[f64]([0.0, 0.0]))[0])  # 1.0
    print("log", sum(math.log(array[f64]([1.0, 1.0]))))  # 0.0
    # trig at 0: sin 0, cos 1, tan 0.
    print(
        "trig",
        math.sin(array[f64]([0.0]))[0],
        math.cos(array[f64]([0.0]))[0],
        math.tan(array[f64]([0.0]))[0],
    )  # 0.0 1.0 0.0
    # fabs over float and int arrays (both float results).
    print("fabs", sum(math.fabs(array[f64]([-3.5, 2.5]))))  # 6.0
    print("fabs_i", sum(math.fabs(array[i64]([-2, 3]))))  # 5.0
    # N-D: shape is preserved.
    auto m = math.sqrt(array[f64]([[4.0, 9.0], [16.0, 25.0]]))
    print("nd", m.ndim, m[1, 1], sum(m))  # 2 5.0 14.0
    # scalar math is unaffected.
    print("scalar", int64(math.sqrt(2.0)))  # 1
