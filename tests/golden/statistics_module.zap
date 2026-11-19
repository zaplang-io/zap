# Golden fixture: the `statistics` module (#980, D-0243, epic #966) — the
# ergonomic scalar-list descriptive-statistics surface (Python `statistics`).
# Exactly-representable results are asserted by value; irrational / sqrt / log
# results go through `math.isclose` so the golden is deterministic across
# platforms. Every result is `float` (D-0243): an int-list input still yields a
# float (e.g. `mode([5, 5, 5, 1])` -> `5.0`). The raising paths (empty / invalid
# input -> the StatisticsError-equivalent `ValueError`) are exercised through
# `try`/`except`. Floats print as the shortest round-trip decimal.
import math
import statistics

def main():
    # --- means (exact where representable) ---
    print(
        "mean",
        statistics.mean([1.0, 2.0, 3.0, 4.0]),
        statistics.mean([1, 2, 3, 4, 5]),
    )
    # fmean is Shewchuk-exact: ten 0.1s average to exactly 0.1 (a naive mean drifts).
    auto tenths = [0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1]
    print("fmean", statistics.fmean([1.0, 2.0, 3.0, 4.0]), statistics.fmean(tenths))
    print("hmean_exact", statistics.harmonic_mean([2.0, 2.0, 2.0]))

    # --- order statistics (exact) ---
    print(
        "median",
        statistics.median([1.0, 2.0, 3.0]),
        statistics.median([1.0, 2.0, 3.0, 4.0]),
    )
    print(
        "median_lohi",
        statistics.median_low([1.0, 2.0, 3.0, 4.0]),
        statistics.median_high([1.0, 2.0, 3.0, 4.0]),
        statistics.median_low([1.0, 2.0, 3.0]),
        statistics.median_high([1.0, 2.0, 3.0]),
    )
    # mode returns the first value with the maximal count (first-appearance order
    # on a tie); an int list still yields a float.
    print(
        "mode",
        statistics.mode([1.0, 1.0, 2.0, 3.0, 3.0, 3.0]),
        statistics.mode([1.0, 1.0, 2.0, 2.0]),
        statistics.mode([5, 5, 5, 1]),
        statistics.mode([1.0, 2.0, 3.0]),
    )
    print("multimode_ties", statistics.multimode([1.0, 1.0, 2.0, 2.0, 3.0]))
    print("multimode_all", statistics.multimode([1.0, 2.0, 3.0]))
    # multimode on empty data is [] (does not raise).
    list[float] empty = []
    print("multimode_empty", statistics.multimode(empty))

    # --- spread (exact) ---
    print(
        "variance",
        statistics.variance([1.0, 2.0, 3.0, 4.0, 5.0]),
        statistics.pvariance([1.0, 2.0, 3.0, 4.0, 5.0]),
    )
    print("variance_xbar", statistics.variance([1.0, 2.0, 3.0], xbar=2.0))
    print("pvariance_mu", statistics.pvariance([2.0, 4.0], mu=3.0))

    # --- quantiles (exact quarters) ---
    print("q_default", statistics.quantiles([1.0, 2.0, 3.0, 4.0]))
    print("q_excl", statistics.quantiles([1.0, 2.0, 3.0, 4.0], n=4, method="exclusive"))
    print("q_incl", statistics.quantiles([1.0, 2.0, 3.0, 4.0], n=4, method="inclusive"))
    # `n=` interior default filled when only `method=` is given by keyword.
    print("q_incl_defn", statistics.quantiles([1.0, 2.0, 3.0, 4.0], method="inclusive"))

    # --- relationships (exact) ---
    print("cov", statistics.covariance([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]))
    print(
        "corr",
        statistics.correlation([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]),
        statistics.correlation([1.0, 2.0, 3.0], [3.0, 2.0, 1.0]),
    )
    auto slope, intercept = statistics.linear_regression(
        [1.0, 2.0, 3.0],
        [2.0, 4.0, 6.0],
    )
    print("linreg", slope, intercept)
    auto pslope, pintercept = statistics.linear_regression(
        [1.0, 2.0],
        [2.0, 4.0],
        proportional=True,
    )
    print("linreg_prop", pslope, pintercept)

    # --- irrational / sqrt / log results via isclose (deterministic) ---
    if math.isclose(statistics.geometric_mean([1.0, 2.0, 4.0, 8.0]), 2.82842712474619):
        print("geomean_ok")
    if math.isclose(statistics.geometric_mean([4.0, 9.0]), 6.0):
        print("geomean_exact_ok")
    if math.isclose(statistics.harmonic_mean([1.0, 2.0, 4.0]), 1.7142857142857142):
        print("hmean_ok")
    auto sd = statistics.stdev([1.0, 2.0, 3.0, 4.0, 5.0])
    auto psd = statistics.pstdev([1.0, 2.0, 3.0, 4.0, 5.0])
    if math.isclose(sd, 1.5811388300841898) and math.isclose(psd, 1.4142135623730951):
        print("stdev_ok")
    if math.isclose(statistics.stdev([1.0, 2.0, 3.0], xbar=2.0), 1.0):
        print("stdev_xbar_ok")

    # --- raising paths (catchable ValueError) ---
    try:
        auto b0 = statistics.mean(empty)
        print(b0)
    except ValueError:
        print("mean_empty")
    try:
        auto b1 = statistics.mode(empty)
        print(b1)
    except ValueError:
        print("mode_empty")
    try:
        auto b2 = statistics.variance([1.0])
        print(b2)
    except ValueError:
        print("variance_one")
    try:
        auto b3 = statistics.stdev([1.0])
        print(b3)
    except ValueError:
        print("stdev_one")
    try:
        auto b4 = statistics.geometric_mean([1.0, -2.0])
        print(b4)
    except ValueError:
        print("geomean_nonpos")
    try:
        auto b5 = statistics.geometric_mean([0.0, 1.0])
        print(b5)
    except ValueError:
        print("geomean_zero")
    try:
        auto b6 = statistics.harmonic_mean([1.0, 0.0])
        print(b6)
    except ValueError:
        print("hmean_nonpos")
    try:
        auto b7 = statistics.quantiles([1.0])
        print(b7)
    except ValueError:
        print("quantiles_one")
    try:
        auto b8 = statistics.quantiles([1.0, 2.0], n=0)
        print(b8)
    except ValueError:
        print("quantiles_n0")
    try:
        auto b9 = statistics.quantiles([1.0, 2.0, 3.0], method="bogus")
        print(b9)
    except ValueError:
        print("quantiles_method")
    try:
        auto b10 = statistics.covariance([1.0], [1.0])
        print(b10)
    except ValueError:
        print("cov_one")
    try:
        auto b11 = statistics.covariance([1.0, 2.0], [1.0, 2.0, 3.0])
        print(b11)
    except ValueError:
        print("cov_mismatch")
    try:
        auto b12 = statistics.correlation([1.0, 1.0, 1.0], [1.0, 2.0, 3.0])
        print(b12)
    except ValueError:
        print("corr_constant")
    try:
        auto b13 = statistics.linear_regression([1.0, 1.0], [1.0, 2.0])
        print(b13)
    except ValueError:
        print("linreg_constant")
