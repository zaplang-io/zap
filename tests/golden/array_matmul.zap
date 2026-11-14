# Golden fixture: matrix multiply `@` / `dot` on numeric arrays (#509, D-0172,
# PEP 465). 1-D·1-D is a scalar dot product; 2-D·2-D a matrix product; a mixed
# 1-D/2-D operand is promoted to a row/column and its singleton axis dropped.
# A shape mismatch raises a catchable ValueError.
def main():
    # 1-D · 1-D -> scalar dot product
    auto a = array[i64]([1, 2, 3])
    auto b = array[i64]([4, 5, 6])
    print(a @ b)  # 1*4 + 2*5 + 3*6 = 32
    print(dot(a, b))  # 32 (function form)

    # 2-D · 2-D -> matrix product
    auto m = array[i64]([[1, 2], [3, 4]])
    auto n = array[i64]([[5, 6], [7, 8]])
    auto p = m @ n  # [[19, 22], [43, 50]]
    print("mm", p.ndim, len(p), p[0, 0], p[0, 1], p[1, 0], p[1, 1])

    # 2-D · 1-D -> 1-D
    auto v = array[i64]([1, 1])
    auto mv = m @ v  # [3, 7]
    print("mv", mv.ndim, len(mv), mv[0], mv[1])

    # 1-D · 2-D -> 1-D
    auto vm = v @ m  # [4, 6]
    print("vm", vm[0], vm[1])

    # float dot product
    auto fa = array[f64]([1.0, 2.0])
    auto fb = array[f64]([3.0, 4.0])
    print(str(fa @ fb))  # 11.0

    # in-place `@=` (identity leaves q unchanged)
    auto q = array[i64]([[2, 3], [4, 5]])
    auto id = array[i64]([[1, 0], [0, 1]])
    q @= id
    print("imm", q[0, 0], q[1, 1], sum(q))  # 2 5 14

    # shape mismatch raises ValueError
    try:
        auto bad = array[i64]([1, 2]) @ array[i64]([1, 2, 3])
        print(bad)
    except ValueError:
        print("mismatch")
