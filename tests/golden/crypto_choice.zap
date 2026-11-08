# Golden fixture: CSPRNG-backed uniform element pick (ticket #767, D-0133).
#
# `crypto.choice(seq: list[T]) -> T` returns a uniformly random element of the
# list, drawn from the OS CSPRNG (mirrors Python secrets.choice, and the surface
# of random.choice). It is element-generic (works for int, str, ... lists). An
# empty list raises a catchable ValueError (unlike random.choice, which aborts).
#
# Random output cannot be pinned to an exact value, so this fixture asserts the
# contract: every draw is a member of the sequence, a single-element list is
# deterministic, full-coverage over many draws (a stuck or biased picker would
# miss a bucket), element-generic behaviour over a str list, and the empty-list
# raise. Intent documentation in code.

def main():
    # Single-element list always returns that element.
    auto one = [42]
    for i in range(0, 50):
        assert crypto.choice(one) == 42

    # Every draw is a member of the sequence, and over many draws every element
    # is hit at least once (uniform CSPRNG, not a stuck or biased value).
    auto xs = [10, 20, 30]
    auto saw10 = False
    auto saw20 = False
    auto saw30 = False
    for i in range(0, 600):
        auto x = crypto.choice(xs)
        assert x == 10 or x == 20 or x == 30
        if x == 10:
            saw10 = True
        if x == 20:
            saw20 = True
        if x == 30:
            saw30 = True
    assert saw10
    assert saw20
    assert saw30

    # Full-coverage count check over a wider list: every bucket is hit.
    auto vals = [0, 1, 2, 3, 4, 5, 6, 7]
    auto counts = [0, 0, 0, 0, 0, 0, 0, 0]
    for i in range(0, 4000):
        auto k = crypto.choice(vals)
        assert k >= 0
        assert k < 8
        counts[k] = counts[k] + 1
    for b in range(0, 8):
        assert counts[b] > 0

    # Element-generic: a str list yields one of its members.
    auto words = ["alpha", "beta", "gamma"]
    auto w = crypto.choice(words)
    assert w == "alpha" or w == "beta" or w == "gamma"

    # An empty list raises a catchable ValueError (not an abort).
    list[int64] empty = []
    auto raised = False
    try:
        crypto.choice(empty)
    except ValueError:
        raised = True
    assert raised

    print("crypto.choice: PASS")
