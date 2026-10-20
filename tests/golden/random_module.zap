# Golden fixture: the `random` stdlib module (ticket #98, D-0056). A seedable
# xoshiro256** generator. Seeding makes every draw deterministic, so this golden
# is reproducible across platforms and both collectors. The first line locks the
# exact generator output (a seeded sum of randint draws); the rest assert the
# structural invariants (ranges, choice membership, shuffle preserves the
# multiset, same seed -> same draw).
import random

def main():
    # Exact generator output: sum of five randint(1, 6) draws from seed 12345.
    random.seed(12345)
    int64 total = 0
    for i in range(0, 5):
        total = total + random.randint(1, 6)
    print(total)

    # random() is in [0, 1), so its integer part is always 0.
    print(int64(random.random()))

    # uniform(a, b) stays within [a, b).
    float64 u = random.uniform(10.0, 20.0)
    if u >= 10.0 and u < 20.0:
        print(1)
    else:
        print(0)

    # choice returns a member of the sequence.
    auto xs = [100, 200, 300]
    int64 c = random.choice(xs)
    if c == 100 or c == 200 or c == 300:
        print(1)
    else:
        print(0)

    # shuffle permutes in place, preserving the multiset (sum is invariant).
    auto ys = [1, 2, 3, 4, 5]
    random.shuffle(ys)
    int64 s = 0
    for y in ys:
        s = s + y
    print(s)

    # Determinism: the same seed yields the same first draw.
    random.seed(999)
    int64 a = random.randint(0, 1000000)
    random.seed(999)
    int64 b = random.randint(0, 1000000)
    if a == b:
        print(1)
    else:
        print(0)
