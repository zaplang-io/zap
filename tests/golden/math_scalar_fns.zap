# Golden fixture: the #973 everyday scalar `math` functions (D-0234). Integer
# math (gcd/lcm/factorial/comb/perm/isqrt) returns exact `int`; the libm float
# additions, tuple decompositions, and list reductions are asserted at exact
# representable values where possible and via `math.isclose` for the irrational
# / round-trip cases (so the golden is deterministic across platforms). The
# raising paths (domain / overflow / length mismatch) are exercised through
# `try`/`except`. Floats print via `str()` as the shortest round-trip decimal.
import math

def main():
    # --- integer math (exact) ---
    print("gcd", math.gcd(0, 0), math.gcd(12, 8), math.gcd(-12, 8), math.gcd(17, 5))
    print("lcm", math.lcm(4, 6), math.lcm(0, 5), math.lcm(21, 6))
    print("fact", math.factorial(0), math.factorial(5), math.factorial(20))
    print(
        "comb",
        math.comb(5, 2),
        math.comb(52, 5),
        math.comb(0, 0),
        math.comb(5, 0),
        math.comb(5, 5),
        math.comb(5, 7),
    )
    print("perm", math.perm(5, 2), math.perm(5, 0), math.perm(5, 5), math.perm(5, 7))
    print("isqrt", math.isqrt(0), math.isqrt(16), math.isqrt(17), math.isqrt(99))

    # --- decompose / compose (exact) ---
    auto frac, ip = math.modf(3.5)
    print("modf", frac, ip)
    auto mant, ex = math.frexp(8.0)
    print("frexp", mant, ex)
    print("ldexp", math.ldexp(0.5, 4))
    print("remainder", math.remainder(7.0, 3.0), math.remainder(5.0, 3.0))
    # Sign propagation, including a runtime -0.0 (copysign(0.0, -5.0)) feeding
    # the third copysign, which must then yield -1.0.
    print(
        "copysign",
        math.copysign(3.0, -1.0),
        math.copysign(-3.0, 1.0),
        math.copysign(1.0, math.copysign(0.0, -5.0)),
    )
    print("nextafter", math.nextafter(1.0, 2.0))

    # --- accurate variants / exp2 (exact where representable) ---
    print("exp2", math.exp2(10.0))
    print("expm1", math.expm1(0.0))
    print("log1p", math.log1p(0.0))

    # --- reductions (exact) ---
    auto xi = [1, 2, 3, 4]
    print("prod_i", math.prod(xi))
    auto xf = [1.0, 2.0, 2.5]
    print("prod_f", math.prod(xf))
    # fsum defeats naive summation: ten 0.1s sum to exactly 1.0.
    auto tenths = [0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1]
    print("fsum_tenths", math.fsum(tenths))
    # A huge-then-cancel sequence a naive sum would lose: the exact total is 0.0.
    auto cancel = [1e100, 1.0, -1e100, -1.0]
    print("fsum_cancel", math.fsum(cancel))
    auto p0 = [0.0, 0.0]
    auto p1 = [3.0, 4.0]
    print("dist", math.dist(p0, p1))
    auto ai = [1, 2, 3]
    auto bi = [4, 5, 6]
    print("sumprod_i", math.sumprod(ai, bi))
    auto af = [1.0, 2.0]
    auto bf = [3.0, 4.0]
    print("sumprod_f", math.sumprod(af, bf))

    # --- irrational / round-trip float checks via isclose (deterministic) ---
    if math.isclose(math.degrees(math.pi), 180.0):
        print("degrees_ok")
    if math.isclose(math.radians(180.0), math.pi):
        print("radians_ok")
    if math.isclose(math.cbrt(27.0), 3.0):
        print("cbrt_ok")
    auto h0 = math.isclose(math.sinh(0.0), 0.0)
    auto h1 = math.isclose(math.cosh(0.0), 1.0)
    auto h2 = math.isclose(math.tanh(0.0), 0.0)
    if h0 and h1 and h2:
        print("hyper_ok")
    auto hi0 = math.isclose(math.asinh(math.sinh(1.0)), 1.0)
    auto hi1 = math.isclose(math.acosh(math.cosh(1.5)), 1.5)
    auto hi2 = math.isclose(math.atanh(math.tanh(0.5)), 0.5)
    if hi0 and hi1 and hi2:
        print("hyper_inv_ok")
    auto g0 = math.isclose(math.gamma(5.0), 24.0)
    auto g1 = math.isclose(math.lgamma(1.0), 0.0, abs_tol=1e-12)
    if g0 and g1:
        print("gamma_ok")
    auto e0 = math.isclose(math.erf(0.0), 0.0, abs_tol=1e-12)
    auto e1 = math.isclose(math.erfc(0.0), 1.0)
    if e0 and e1:
        print("erf_ok")

    # --- classification ---
    if math.isfinite(1.0) and not math.isfinite(math.inf) and not math.isfinite(math.nan):
        print("isfinite_ok")

    # --- isclose semantics ---
    auto c0 = math.isclose(1.0, 1.0)
    auto c1 = math.isclose(1.0, 1.000000000001)
    auto c2 = math.isclose(1.0, 1.1)
    if c0 and c1 and not c2:
        print("isclose_basic_ok")
    auto t0 = math.isclose(1.0, 1.1, abs_tol=0.2)
    auto t1 = math.isclose(1.0, 1.1, rel_tol=0.05)
    if t0 and not t1:
        print("isclose_tol_ok")
    auto s0 = math.isclose(math.inf, math.inf)
    auto s1 = math.isclose(math.inf, -math.inf)
    auto s2 = math.isclose(math.nan, math.nan)
    if s0 and not s1 and not s2:
        print("isclose_special_ok")

    # --- raising paths (catchable) ---
    try:
        auto bad = math.factorial(-1)
        print(bad)
    except ValueError:
        print("factorial_neg")
    try:
        auto bad2 = math.factorial(21)
        print(bad2)
    except ArithmeticError:
        print("factorial_overflow")
    try:
        auto bad3 = math.isqrt(-4)
        print(bad3)
    except ValueError:
        print("isqrt_neg")
    try:
        auto bad4 = math.comb(-1, 2)
        print(bad4)
    except ValueError:
        print("comb_neg")
    try:
        auto short = [1.0]
        auto longer = [1.0, 2.0]
        auto bad5 = math.dist(short, longer)
        print(bad5)
    except ValueError:
        print("dist_mismatch")
    try:
        auto bad6 = math.isclose(1.0, 1.0, rel_tol=-1.0)
        print(bad6)
    except ValueError:
        print("isclose_negtol")
