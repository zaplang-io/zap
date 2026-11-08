# Golden fixture: unbiased bounded random integer (ticket #767, D-0133).
#
# `crypto.randbelow(n) -> int` returns a uniformly random integer in [0, n),
# drawn from the OS CSPRNG with rejection sampling to avoid modulo bias (mirrors
# Python secrets.randbelow). Random output cannot be pinned to an exact value,
# so this fixture asserts the contract: the range bound, randbelow(1) == 0, a
# no-modulo-bias coverage smoke check (every bucket of a small n is hit across
# many draws), and that n <= 0 raises. (crypto.choice, which builds on this, has
# its own fixture crypto_choice.zap.)

def main():
    # randbelow(1) is always 0.
    for i in range(0, 50):
        assert crypto.randbelow(1) == 0

    # randbelow(n) is always in [0, n) over many draws, for several n.
    for i in range(0, 2000):
        auto r = crypto.randbelow(7)
        assert r >= 0
        assert r < 7
        auto big = crypto.randbelow(1000000)
        assert big >= 0
        assert big < 1000000

    # No-modulo-bias coverage smoke check: over many draws of a small n, every
    # bucket in [0, n) is hit at least once (a biased or broken sampler would
    # systematically miss or crowd buckets).
    auto counts = [0, 0, 0, 0, 0, 0]
    for i in range(0, 3000):
        auto k = crypto.randbelow(6)
        counts[k] = counts[k] + 1
    for b in range(0, 6):
        assert counts[b] > 0

    # n <= 0 raises a catchable ValueError (zero and negative).
    auto raised = False
    try:
        crypto.randbelow(0)
    except ValueError:
        raised = True
    assert raised
    raised = False
    try:
        crypto.randbelow(-5)
    except ValueError:
        raised = True
    assert raised

    print("crypto.randbelow: PASS")
