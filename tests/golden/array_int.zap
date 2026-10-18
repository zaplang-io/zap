# Golden fixture: 1-D numeric arrays, i64 dtype (ticket #24).
# Construction, element-wise +/-/*, scalar broadcast, sum reduction, len, index.
# The element-wise ops lower to LLVM vector instructions (SIMD) in codegen.
def main():
    auto a = array[i64]([1, 2, 3, 4, 5, 6, 7, 8])
    auto b = array[i64]([8, 7, 6, 5, 4, 3, 2, 1])
    print(sum(a + b))      # each 9 -> 72
    print(sum(a * b))      # 8,14,18,20,20,18,14,8 -> 120
    print(sum(a - b))      # -7,-5,-3,-1,1,3,5,7 -> 0
    print(sum(a + 10))     # 11..18 -> 116
    print(sum(a * 2))      # 2,4,..,16 -> 72
    print(sum(10 + a))     # scalar-on-left broadcast -> 116
    print(len(a))          # 8
    print(a[0])            # 1
    auto c = a + b
    print(c[3])            # 9
