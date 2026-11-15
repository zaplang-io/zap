# claim: ARR-25
# SPEC §15 (#933): the numeric-array sort accelerator — large contiguous unstable
# sorts dispatch to the vendored SIMD kernels (x86-simd-sort on x86, Highway
# vqsort on aarch64) via cpu_features, falling back to the own introsort floor
# (#932) for small / strided / stable / other-dtype sorts and on CPUs without the
# ISA. A performance change only: a sort is a permutation, so results are
# identical to the floor / NumPy (NaNs to the end). Verified vs NumPy 2.5.4.
import random

def main():
    auto a = random.integers(random.default_rng(42), 0, 1000000, size=1000)
    a.sort()
    print("isort", str(a[0]), str(a[999]))  # 4379 999104
    auto a2 = random.integers(random.default_rng(42), 0, 1000000, size=1000)
    auto ix = argsort(a2)
    print("argsort", str(a2[ix[0]]), str(a2[ix[999]]))  # 4379 999104
