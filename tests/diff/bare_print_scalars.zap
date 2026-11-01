# diff: match
# Bare `print(x)` renders each scalar via `str(x)`, matching CPython per
# argument (I#493). Before the fix Zap's `print` used C `printf` formats
# directly, so `print(27.5)` gave `27.500000` and `print(True)` gave `-1` — the
# reason the corpus wraps floats/bools in `str()` (see float_str_integral.zap).
# With `print` unified onto the `str()` path, the bare forms now match CPython:
# `27.5`, `6.0` (integral float keeps `.0`, like CPython), `-4.5`, `True`,
# `False`, `42`, `hello`, and the space-joined multi-argument line.
def main():
    print(27.5)
    print(6.0)
    print(-4.5)
    print(True)
    print(False)
    print(42)
    print(-7)
    print("hello")
    print(1.5, True, 42)
