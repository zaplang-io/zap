# Golden fixture: the full numeric dtype matrix (#503 f32/i32, #890 i8/i16 +
# u16/u32/u64). Each dtype constructs, indexes, reduces, and does element-wise
# arithmetic; narrow integer dtypes wrap at their own width (D-0005).
def main():
    # f32: element get widens to f64; construction/arith narrow back to f32.
    auto f = array[f32]([1.5, 2.5, 3.0])
    print("f32", f[0], sum(f), sum(f + f), sum(f * 2.0))  # 1.5 7.0 14.0 14.0

    # i32 signed, no wrap at these magnitudes.
    auto i32a = array[i32]([1, 2, 3, 4])
    print("i32", sum(i32a), sum(i32a + i32a), i32a[3])  # 10 20 4

    # i8 wraps mod 256: 127+10 -> -119, -128+(-10) -> 118.
    auto i8sum = array[i8]([127, -128]) + array[i8]([10, -10])
    print("i8wrap", i8sum[0], i8sum[1])  # -119 118

    # u16 wraps mod 65536: 65535+1 -> 0; sum accumulates in int64 (no wrap).
    auto u16a = array[u16]([65535, 1])
    print("u16", (u16a + array[u16]([1, 0]))[0], sum(u16a))  # 0 65536

    # u32 / u64 unsigned.
    auto u32a = array[u32]([10, 20, 30])
    print("u32", sum(u32a), u32a[1])  # 60 20
    auto u64a = array[u64]([100, 200])
    print("u64", sum(u64a), u64a[0])  # 300 100

    # i16 N-D construction + multi-axis index.
    auto m = array[i16]([[1, 2, 3], [4, 5, 6]])
    print("i16nd", m[1, 2], m.size, sum(m))  # 6 6 21
