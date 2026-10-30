# Golden fixture: list slicing (#400). A list slice is a COPY (D-0030) with
# Python bounds — negatives count from the end, out-of-range clamps (no fault,
# unlike indexing), and the step may be negative. Mutating the slice leaves the
# original untouched. Reduced to scalar prints so the golden does not depend on
# list repr.
def main():
    auto xs = [10, 20, 30, 40, 50]
    auto head = xs[1:3]
    print(len(head))
    print(head[0])
    print(head[1])
    print(xs[::2][2])
    print(xs[::-1][0])
    print(xs[-2:][0])
    print(len(xs[:2]))
    print(len(xs[3:]))
    print(len(xs[1:100]))
    print(len(xs[3:1]))
    head[0] = 99
    print(head[0])
    print(xs[1])
    print(len(xs))
