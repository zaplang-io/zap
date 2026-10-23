# Golden fixture: 1-D numeric arrays, f64 dtype (ticket #24).
# Five-element arrays exercise both the vector body (4 lanes) and the scalar
# remainder loop. Element-wise math lowers to <4 x double> SIMD ops in codegen.
def main():
    auto a = array[f64]([1.0, 2.0, 3.0, 4.0, 5.0])
    auto b = array[f64]([5.0, 4.0, 3.0, 2.0, 1.0])
    print(sum(a + b))  # each 6.0 -> 30.0
    print(sum(a * b))  # 5,8,9,8,5 -> 35.0
    print(sum(a * 2.0))  # 2,4,6,8,10 -> 30.0
    print(sum(a + 1.0))  # 2,3,4,5,6 -> 20.0
    print(a[2])  # 3.0
