# Golden fixture: the NumPy-compatible bit-exact PCG64 `Generator` (#930/#931,
# D-0189). Every value here is `numpy.random.default_rng(42).<method>(...)`
# reproduced VALUE-FOR-VALUE (SeedSequence + PCG64 + Lemire integers + ziggurat /
# rejection distributions + choice/shuffle), so this fixture is the bit-exact
# contract AND a walled garden around the stream: a change that perturbs any draw
# breaks it. A fresh `default_rng(42)` per line keeps each independently checkable
# against NumPy. Integer-valued draws (integers/poisson/binomial/choice/shuffle/
# permutation) pin the engine exactly; float draws pin the ziggurat/rejection
# transforms (printed at Zap's shortest round-trip repr, == NumPy/Python repr).
import random

def main():
    # integers: buffered Lemire (32-bit for ranges <= 2^32, 64-bit above).
    auto ia = random.integers(random.default_rng(42), 0, 10, size=8)
    print("int", str(ia[0]), str(ia[1]), str(ia[2]), str(ia[7]))  # 0 7 6 6
    auto ib = random.integers(random.default_rng(42), 0, 10, size=8, endpoint=True)
    print("intend", str(ib[0]), str(ib[1]), str(ib[5]))  # 0 8 9
    auto ic = random.integers(random.default_rng(42), -5, 5, size=6)
    print("intneg", str(ic[0]), str(ic[1]), str(ic[2]))  # -5 2 1
    auto idd = random.integers(random.default_rng(42), 0, 1099511627776, size=4)
    print("intbig", str(idd[0]), str(idd[1]))  # 850973674774 482551947687
    print("intscalar", str(random.integers(random.default_rng(42), 0, 1000000)))  # 89250

    # uniform floats.
    print("random", str(random.random(random.default_rng(42))))  # 0.7739560485559633
    auto ra = random.random(random.default_rng(42), size=5)
    print("randarr", str(ra[0]), str(ra[1]), str(ra[4]))  # 0.773... 0.438... 0.094...
    print("uniform", str(random.uniform(random.default_rng(42), -3.0, 7.0)))  # 4.739560485559633
    auto ua = random.uniform(random.default_rng(42), -3.0, 7.0, size=5)
    print("uniarr", str(ua[0]), str(ua[4]))  # 4.739... -2.058...

    # normal / exponential / gamma / beta (ziggurat + rejection).
    print("stdnormal", str(random.standard_normal(random.default_rng(42))))  # 0.30471707975443135
    auto sn = random.standard_normal(random.default_rng(42), size=4)
    print("stdnarr", str(sn[0]), str(sn[1]), str(sn[3]))  # 0.304... -1.039... 0.940...
    auto nm = random.normal(random.default_rng(42), 2.0, 3.0, size=6)
    print("normal", str(nm[0]), str(nm[5]))  # 2.914... -1.906...
    print("stdexp", str(random.standard_exponential(random.default_rng(42))))  # 2.4042086039659947
    auto ex = random.exponential(random.default_rng(42), 2.5, size=4)
    print("exp", str(ex[0]), str(ex[3]))  # 6.010... 0.699...
    auto ga = random.standard_gamma(random.default_rng(42), 0.5, size=4)
    print("gammaA", str(ga[0]), str(ga[3]))  # 0.804... 0.755...
    auto gb = random.standard_gamma(random.default_rng(42), 2.5, size=4)
    print("gammaB", str(gb[0]), str(gb[3]))  # 2.646... 2.142...
    auto gc = random.gamma(random.default_rng(42), 2.5, 2.0, size=4)
    print("gammaC", str(gc[0]), str(gc[3]))  # 5.293... 4.284...
    auto ba = random.beta(random.default_rng(42), 0.5, 0.8, size=4)
    print("betaA", str(ba[0]), str(ba[3]))  # 0.626... 0.556...
    auto bb = random.beta(random.default_rng(42), 2.0, 5.0, size=4)
    print("betaB", str(bb[0]), str(bb[3]))  # 0.243... 0.298...

    # discrete: poisson (mult for lam<10, PTRS for lam>=10), binomial
    # (inversion for n*p<=30, BTPE above).
    auto p3 = random.poisson(random.default_rng(42), 3.0, size=8)
    print("pois3", str(p3[0]), str(p3[1]), str(p3[7]))  # 4 4 2
    auto p20 = random.poisson(random.default_rng(42), 20.0, size=8)
    print("pois20", str(p20[0]), str(p20[1]), str(p20[7]))  # 24 14 24
    print("poisscalar", str(random.poisson(random.default_rng(42), 5.0)))  # 8
    auto bnA = random.binomial(random.default_rng(42), 10, 0.3, size=8)
    print("binomA", str(bnA[0]), str(bnA[1]), str(bnA[7]))  # 4 3 4
    auto bnB = random.binomial(random.default_rng(42), 1000, 0.4, size=8)
    print("binomB", str(bnB[0]), str(bnB[1]), str(bnB[7]))  # 390 411 423
    auto bnC = random.binomial(random.default_rng(42), 20, 0.8, size=8)
    print("binomC", str(bnC[0]), str(bnC[7]))  # 15 15

    # choice over an int population: replace / no-replace (Floyd) / p=.
    auto chA = random.choice(random.default_rng(42), 8, size=6)
    print("choiceA", str(chA[0]), str(chA[1]), str(chA[5]))  # 0 6 6
    auto chB = random.choice(random.default_rng(42), 10, size=4, replace=False)
    print("choiceB", str(chB[0]), str(chB[1]), str(chB[3]))  # 4 0 6
    auto chC = random.choice(
        random.default_rng(42),
        5,
        size=8,
        p=array[f64]([0.1, 0.2, 0.3, 0.25, 0.15]),
    )
    print("choiceC", str(chC[0]), str(chC[1]), str(chC[7]))  # 3 2 3
    print("choiceScalar", str(random.choice(random.default_rng(42), 100)))  # 8

    # shuffle (in place) / permutation (int -> shuffled arange). Both are the
    # same random_interval Fisher-Yates over arange(10), so both give 5 6 ... 8.
    auto sh = array[i64]([0, 1, 2, 3, 4, 5, 6, 7, 8, 9])
    random.shuffle(random.default_rng(42), sh)
    print("shuffle", str(sh[0]), str(sh[1]), str(sh[9]))  # 5 6 8
    auto pm = random.permutation(random.default_rng(42), 10)
    print("perm", str(pm[0]), str(pm[1]), str(pm[9]))  # 5 6 8

    # N-D size: a 2x3 array of standard normals (row-major sequential draws).
    auto nd = random.normal(random.default_rng(42), 0.0, 1.0, size=(2, 3))
    print(
        "ndsize",
        str(nd.ndim),
        str(nd.shape[0]),
        str(nd.shape[1]),
        str(nd[0, 0]),
        str(nd[1, 2]),
    )  # 2 2 3 0.304... -1.302...
