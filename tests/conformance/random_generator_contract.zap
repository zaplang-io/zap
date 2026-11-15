# claim: ARR-22
# SPEC §15 (#930/#931): `import random` — NumPy-compatible bit-exact PCG64
# `Generator`. `default_rng(seed)` -> a handle; the draws reproduce
# `numpy.random.default_rng(seed).<method>(...)` value-for-value. `random` /
# `uniform` / `integers` (int or N-D `size=`, `endpoint=`); `normal` /
# `standard_normal` / `exponential` / `gamma` / `beta` (ziggurat + rejection);
# `poisson` (mult / PTRS) / `binomial` (inversion / BTPE); `choice` (replace /
# no-replace / p=), `shuffle`, `permutation`. Verified against NumPy 2.5.4.
import random

def main():
    auto ia = random.integers(random.default_rng(42), 0, 10, size=5)
    print("int", str(ia[0]), str(ia[1]), str(ia[4]), str(len(ia)))  # 0 7 4 5
    auto ib = random.integers(random.default_rng(42), 0, 10, size=5, endpoint=True)
    print("intend", str(ib[0]), str(ib[1]))  # 0 8
    print("random", str(random.random(random.default_rng(42))))  # 0.7739560485559633
    print("uniform", str(random.uniform(random.default_rng(42), -3.0, 7.0)))  # 4.739560485559633
    auto nm = random.standard_normal(random.default_rng(42), size=3)
    print("normal", str(nm[0]), str(nm[2]))  # 0.3047... 0.7504...
    auto p3 = random.poisson(random.default_rng(42), 3.0, size=5)
    print("pois", str(p3[0]), str(p3[1]), str(p3[4]))  # 4 4 7
    auto p20 = random.poisson(random.default_rng(42), 20.0, size=5)
    print("poisbig", str(p20[0]), str(p20[4]))  # 24 19
    auto bn = random.binomial(random.default_rng(42), 10, 0.3, size=5)
    print("binom", str(bn[0]), str(bn[4]))  # 4 1
    auto bnb = random.binomial(random.default_rng(42), 1000, 0.4, size=5)
    print("binombig", str(bnb[0]), str(bnb[4]))  # 390 388
    auto cr = random.choice(random.default_rng(42), 8, size=6)
    print("chrepl", str(cr[0]), str(cr[5]))  # 0 6
    auto cn = random.choice(random.default_rng(42), 10, size=4, replace=False)
    print("chnorepl", str(cn[0]), str(cn[3]))  # 4 6
    auto pm = random.permutation(random.default_rng(42), 10)
    print("perm", str(pm[0]), str(pm[9]), str(len(pm)))  # 5 8 10
    auto nd = random.normal(random.default_rng(42), 0.0, 1.0, size=(2, 3))
    print("nd", str(nd.ndim), str(nd.shape[0]), str(nd.shape[1]), str(nd[0, 0]))  # 2 2 3 0.3047...
