# claim: ARR-4
# SPEC §15 (#503/#890): the full numeric dtype matrix — floats f32/f64, signed
# i8/i16/i32/i64, unsigned u8/u16/u32/u64, bool — with NEP-50 promotion. Narrow
# integer dtypes wrap at their own width (D-0005); mixed dtypes promote to the
# dtype that holds both (bool+bool -> uint8; int+f32 stays f32).
def main():
    print("f32", sum(array[f32]([1.5, 2.5, 3.0])))
    print("i16", sum(array[i16]([[1, 2], [3, 4]])))
    print("u32", sum(array[u32]([10, 20, 30])))
    print("i8wrap", (array[i8]([127]) + array[i8]([10]))[0])
    print("u16wrap", (array[u16]([65535]) + array[u16]([1]))[0])
    print("u8_i8", (array[u8]([200]) + array[i8]([100]))[0])
    print("i64_f32", (array[i64]([2]) + array[f32]([0.25]))[0])
    print("bool", sum(array[bool]([True, True]) + array[bool]([True, False])))
