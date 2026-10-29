# diff: match
# Negative list indexing: `xs[-1]` counts from the end, matching CPython and
# strings (see str_scalar / string_slice). `xs[-len]` is the first element;
# a negative index past the start faults (catchably, #383) like positive OOB.
# Closed the #398 (conformance IDX-1) gap; CPython: 50, 40, 10.
def main():
    list[int64] xs = [10, 20, 30, 40, 50]
    print(xs[-1])
    print(xs[-2])
    print(xs[-5])
