# diff: diverge D-0038: default arguments are evaluated per call (a fresh value
# each time), not once at def time. CPython evaluates a default once and reuses
# the SAME object across calls, so a mutable default like [] accumulates; Zap
# makes it fresh each call. This is an intentional divergence (it fixes Python's
# classic mutable-default footgun). CPython: 1,2,3; Zap: 1,1,1.
def app(int64 x, list[int64] acc = []) -> int64:
    acc.append(x)
    return len(acc)


def main():
    print(app(1))
    print(app(2))
    print(app(3))
