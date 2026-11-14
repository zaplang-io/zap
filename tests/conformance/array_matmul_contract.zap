# claim: ARR-10
# SPEC §15 (#509): matrix multiply `@` / `dot` on 1-D/2-D numeric arrays. 1-D·1-D
# -> scalar dot product; 2-D·2-D -> matrix product; a mixed 1-D/2-D operand is a
# vector (row/column, singleton axis dropped); NEP-50 result dtype; a shape
# mismatch on the contracted axis raises a catchable ValueError.
def main():
    auto a = array[i64]([1, 2, 3])
    auto b = array[i64]([4, 5, 6])
    print("dot", a @ b, dot(a, b))
    auto m = array[i64]([[1, 2], [3, 4]])
    auto n = array[i64]([[5, 6], [7, 8]])
    print("mm", sum(m @ n))
    print("mv", sum(m @ array[i64]([1, 1])))
    print("fdot", str(array[f64]([1.0, 2.0]) @ array[f64]([3.0, 4.0])))
    try:
        print(sum(m @ array[i64]([[1], [2], [3]])))
    except ValueError:
        print("mismatch")
