# diff: match
# List construction, length, and in-range positive indexing (by literal and by
# variable) — the supported list surface that matches CPython. (Negative
# indexing and slicing are separate documented gaps — see list_negative_index.zap
# and list_slice.zap.)
def main():
    list[int64] xs = [10, 20, 30, 40, 50]
    print(len(xs))
    print(xs[0])
    print(xs[4])
    print(xs[0] + xs[4])
    auto total = 0
    for i in range(0, len(xs)):
        total = total + xs[i]
    print(total)
    xs.append(60)
    print(len(xs))
    print(xs[5])
