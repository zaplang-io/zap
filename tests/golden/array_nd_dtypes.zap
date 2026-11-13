# Golden fixture: N-D numeric arrays across dtypes (#500, D-0164).
# N-D shape + multi-axis indexing works for every array dtype; narrow dtypes keep
# their 1-byte storage and wrapping arithmetic (D-0005), and the full `sum`
# reduction and element-wise kernels run over the whole shape.
def main():
    # f64 2-D: scalar get, element-wise, full reduction.
    auto f = array[f64]([[1.0, 2.0], [3.0, 4.0]])
    print("f", f[0, 0], f[1, 1])       # 1.0 4.0
    print("fadd", sum(f + f))          # 2+4+6+8 -> 20.0
    print("fmul", sum(f * 2.0))        # 2+4+6+8 -> 20.0

    # uint8 2-D (image-like): scalar get stays an int word, array arithmetic wraps
    # mod 256, scalar get + int is plain int64 arithmetic (no wrap).
    auto img = array[uint8]([[250, 10], [5, 1]])
    print("img", img[0, 0], img[1, 0]) # 250 5
    print("isum", sum(img))            # 250+10+5+1 -> 266
    print("iscalar", img[0, 0] + 10)   # plain int64 250+10 -> 260
    auto wrapped = img + 10            # uint8 wrap: 260->4, 20, 15, 11
    print("iwrap", sum(wrapped))       # 4+20+15+11 -> 50

    # bool 2-D mask: arithmetic promotes to uint8, `sum` counts the set bits.
    auto mask = array[bool]([[True, False], [True, True]])
    print("mask", mask[0, 0], mask[0, 1])  # 1 0
    print("msum", sum(mask))               # 3
