# Golden fixture: the explicit `math.fma(x, y, z)` builtin (#946, D-0196) — a
# single rounding of `x*y + z`, bit-identical across targets (hardware FMA where
# present, correctly-rounded libm `fma` otherwise), independent of the implicit
# `--fp` contraction policy. Values chosen to be exactly representable so the
# output is deterministic on every platform.
import math

def main():
    print(math.fma(2.0, 3.0, 4.0))  # 10.0
    print(math.fma(0.5, 0.5, 0.0))  # 0.25
    print(math.fma(-1.0, 1.0, 1.0))  # 0.0
    print(math.fma(1.5, 2.0, 0.5))  # 3.5
    print(int64(math.fma(10.0, 10.0, 5.0)))  # 105
