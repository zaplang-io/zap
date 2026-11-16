# Golden fixture: sliced assignment `xs[lo:hi:step] = ys` (#1005, D-0030).
#
# A step-1 slice splices (same-length replace, shrink, grow, pure insert, full
# `[:]` replace); an extended slice (step != 1) assigns element-wise and raises
# ValueError on a length mismatch. Pointer elements (str) splice like any other.
def main():
    auto a = [1, 2, 3, 4, 5]
    a[1:3] = [20, 30]  # same length -> [1, 20, 30, 4, 5]
    print(a[1])  # 20
    print(len(a))  # 5
    a[1:3] = [9]  # shrink -> [1, 9, 4, 5]
    print(len(a))  # 4
    print(a[2])  # 4
    a[1:1] = [7, 8]  # pure insert at 1 -> [1, 7, 8, 9, 4, 5]
    print(len(a))  # 6
    print(a[1])  # 7
    print(a[2])  # 8
    a[:] = [100, 200]  # full replace -> [100, 200]
    print(len(a))  # 2
    print(a[0])  # 100

    auto b = [0, 1, 2, 3, 4, 5]
    b[0:6:2] = [10, 12, 14]  # extended slice, indices 0/2/4 -> [10, 1, 12, 3, 14, 5]
    print(b[0])  # 10
    print(b[2])  # 12
    print(b[4])  # 14
    print(b[1])  # 1

    auto s = ["a", "b", "c"]
    s[1:2] = ["x", "y"]  # grow with pointer elements -> [a, x, y, c]
    print(s[1])  # x
    print(s[2])  # y
    print(len(s))  # 4

    auto c = [1, 2, 3, 4]
    try:
        c[0:4:2] = [9]  # 2 targets, 1 value -> ValueError
        print("no-raise")
    except ValueError:
        print("caught")  # caught
