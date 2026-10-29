# diff: diverge GAP: `x in <list>` is not supported by codegen ("'in' is only
# supported with a set on the right"), so a program using it fails to compile
# (exit 1) rather than testing membership. CPython supports list membership. Dict
# and string `in` DO work (see dict_lookup.zap) — only list `in` is missing. A
# documented gap to triage. CPython: True / False; Zap: compile error.
def mark(bool b) -> int64:
    if b:
        return 1
    return 0


def main():
    list[int64] xs = [5, 6, 7, 8]
    print(mark(7 in xs))
    print(mark(99 in xs))
