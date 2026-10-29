# diff: diverge GAP: list/array slicing is not yet implemented by codegen
# ("slicing is only supported on str and bytes values (list/array slicing is a
# follow-up)"), so a program slicing a list fails to compile (exit 1). CPython
# slices lists (copy, with clamping and steps). STRING slicing works and matches
# (see string_slice.zap) — this is the list half. A documented gap to triage.
# CPython prints the slice elements/lengths; Zap: compile error.
def main():
    list[int64] xs = [10, 20, 30, 40, 50]
    auto head = xs[1:3]
    print(len(head))
    print(head[0])
    print(len(xs[::2]))
