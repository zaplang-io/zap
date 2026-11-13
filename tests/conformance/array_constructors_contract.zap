# claim: ARR-5
# SPEC §15 (#504): array constructors. zeros/ones/full take an int (1-D) or a
# tuple-literal (N-D) shape; arange builds a half-open range (runtime length, a
# zero step raises); linspace builds num evenly-spaced samples, both ends
# inclusive.
def main():
    print("zeros", sum(zeros[i64]((2, 3))))
    print("ones", sum(ones[f64]((2, 2))))
    print("full", sum(full[i64]((2, 2), 7)))
    print("arange", len(arange[i64](0, 10, 2)), sum(arange[i64](0, 10, 2)))
    print("linspace", linspace[f64](0.0, 1.0, 5)[2], linspace[f64](0.0, 1.0, 5)[4])
    try:
        auto bad = arange[i64](0, 10, 0)
        print(len(bad))
    except ValueError:
        print("step error")
