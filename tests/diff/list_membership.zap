# diff: match
# List membership: `x in <list>` is a linear scan comparing each element for
# equality, matching CPython (dict and string `in` already worked — see
# dict_lookup.zap). Closed the #399 (conformance OP-IN-2) gap. CPython and Zap
# both print: 1 / 0.
def mark(bool b) -> int64:
    if b:
        return 1
    return 0


def main():
    list[int64] xs = [5, 6, 7, 8]
    print(mark(7 in xs))
    print(mark(99 in xs))
