# claim: IDX-1
# SPEC §9.1 (#398): a negative list index counts from the end, exactly like a
# string index (STR-2). `xs[-1]` is the last element, `xs[-len]` the first;
# positive indices are unchanged, and a negative index past the start is a
# catchable out-of-range fault (#383), just like positive OOB.
def main():
    list[int64] xs = [10, 20, 30, 40, 50]
    print(xs[-1])
    print(xs[-2])
    print(xs[-5])
    print(xs[0])
    print(xs[4])
    # negative index also addresses the store side
    xs[-1] = 99
    print(xs[4])
    xs[-5] = 11
    print(xs[0])
