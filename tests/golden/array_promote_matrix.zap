# Golden fixture: NEP-50 dtype promotion (D-0166), observable through values that
# only come out right under the correctly-promoted result dtype.
def main():
    # u8 + i8 -> i16: 200 + 100 = 300 (a u8 result would wrap to 44).
    print("u8_i8", (array[u8]([200]) + array[i8]([100]))[0])  # 300

    # u16 + i16 -> i32: 60000 + 10000 = 70000 (overflows u16/i16, fits i32).
    print("u16_i16", (array[u16]([60000]) + array[i16]([10000]))[0])  # 70000

    # u32 + i32 -> i64: 4000000000 + 1 = 4000000001 (overflows both 32-bit).
    print("u32_i32", (array[u32]([4000000000]) + array[i32]([1]))[0])  # 4000000001

    # int + f32 -> f32 (an int never bumps f32 to f64).
    print("i32_f32", sum(array[i32]([1, 2, 3]) + array[f32]([0.5, 0.5, 0.5])))  # 7.5
    print("i64_f32", (array[i64]([2]) + array[f32]([0.25]))[0])  # 2.25

    # uint8 + i64 -> i64 (legacy promotion preserved).
    print("u8_i64", (array[uint8]([250]) + array[i64]([1000]))[0])  # 1250

    # f32 + f64 -> f64.
    print("f32_f64", (array[f32]([1.5]) + array[f64]([0.25]))[0])  # 1.75

    # bool + bool -> uint8 (True + True == 2, not a bool).
    print("bool_bool", sum(array[bool]([True, True]) + array[bool]([True, False])))  # 3
