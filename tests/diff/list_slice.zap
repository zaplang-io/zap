# diff: match
# List slicing (D-0030): a list slice is a COPY with Python bounds — negatives
# count from the end, out-of-range clamps (no fault, unlike indexing), and the
# step may be negative. Reduced to element/length scalars so the comparison does
# not depend on list repr. Closed the #400 (conformance SLICE-1) gap; string
# slicing already matched (string_slice.zap) — this is the list half.
def main():
    list[int64] xs = [10, 20, 30, 40, 50]
    auto head = xs[1:3]
    print(len(head))
    print(head[0])
    print(head[1])
    auto ev = xs[::2]
    print(len(ev))
    print(ev[2])
    auto rev = xs[::-1]
    print(rev[0])
    print(len(xs[:2]))
    print(len(xs[3:]))
    auto tail = xs[-2:]
    print(tail[0])
    print(len(xs[1:100]))
    print(len(xs[3:1]))
    print(len(xs))
