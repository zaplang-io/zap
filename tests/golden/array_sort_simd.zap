# Golden fixture: the SIMD sort accelerator (#933, D-0195) over LARGE contiguous
# arrays — the path that routes to the vendored x86-simd-sort (x86) / Highway
# vqsort (aarch64) kernels when the CI host supports the ISA, and to the own
# introsort floor (#932) otherwise. Either way a sort is a permutation, so the
# results are identical; these are pinned to numpy. (The #932 goldens use small
# arrays below the accelerator's size threshold, so this one covers the accel.)
import random

def main():
    # 1000-element contiguous array[i64] (>= the accel threshold): unstable sort.
    auto a = random.integers(random.default_rng(42), 0, 1000000, size=1000)
    a.sort()
    print("isort", str(a[0]), str(a[1]), str(a[500]), str(a[999]))  # 4379 4890 506329 999104

    # argsort over the same data: a2[ix[0]] is the min, a2[ix[999]] the max
    # (deterministic regardless of the unstable tie order).
    auto a2 = random.integers(random.default_rng(42), 0, 1000000, size=1000)
    auto ix = argsort(a2)
    print("argsort", str(a2[ix[0]]), str(a2[ix[999]]))  # 4379 999104

    # 500-element f64 sort (the accelerated float path; NaNs would sort to the
    # end on both the accel and the floor).
    auto f = random.random(random.default_rng(7), size=500)
    f.sort()
    print("fsort", str(f[0]), str(f[250]), str(f[499]))  # 0.00373... 0.52116... 0.99905...
