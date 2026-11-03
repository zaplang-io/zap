# claim: FN-8
# SPEC §7 (#601): a `V **name` keyword-collector gathers the extra keyword
# arguments into a homogeneous `dict[str, V]`, which the body treats as an
# ordinary dict. Summing the collected values is order-independent, so the
# expected output does not depend on dict iteration order.
def tally(int64 base, int64 **counts) -> int64:
    auto total = base
    for k in counts.keys():
        total = total + counts[k]
    return total


def main():
    print(tally(0))
    print(tally(0, a=1, b=2, c=3))
    print(tally(10, x=5, y=5))
