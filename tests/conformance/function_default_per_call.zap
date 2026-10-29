# claim: FN-1
# SPEC §7 (D-0038): default arguments are evaluated PER CALL (a fresh value each
# time), not once at definition time. This is a DELIBERATE divergence from Python
# (whose once-at-def default makes a mutable `[]` accumulate across calls — the
# classic footgun). Each call here gets a fresh list, so every call returns 1;
# Python would print 1, 2, 3.
def push(int64 x, list[int64] acc = []) -> int64:
    acc.append(x)
    return len(acc)


def main():
    print(push(1))
    print(push(2))
    print(push(3))
