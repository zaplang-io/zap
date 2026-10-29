# diff: diverge GAP: a negative list index is not interpreted as "from the end".
# CPython reads `xs[-1]` as the last element; Zap rejects/faults on the negative
# index ("list index -1 out of range") rather than wrapping. Positive indexing
# works and matches (see list_index.zap); string negative indexing also works
# (string_slice.zap) — this is the list half. A documented gap to triage.
# CPython: 50, 40, 10; Zap: index error.
def main():
    list[int64] xs = [10, 20, 30, 40, 50]
    print(xs[-1])
    print(xs[-2])
    print(xs[-5])
