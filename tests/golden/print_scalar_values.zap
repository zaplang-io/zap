# Golden fixture: bare `print(x)` renders each scalar exactly as `str(x)` does
# (I#493). Before the fix, `print` lowered scalars straight to C `printf` format
# strings: `%f` rendered a float with a fixed six decimals (`27.5` -> `27.500000`)
# and a bool was widened to an int and printed as its all-ones bitpattern
# (`True` -> `-1`). Bare `print` now routes floats and bools through the same
# `str()` lowering, so a float prints `%g`-style (an integral float keeps its
# `.0`, I#384) and a bool prints `True`/`False`. `int` and `str` already matched
# `str()` and are pinned here as a no-regression guard, as is the space-joined
# multi-argument line.
def main():
    print(27.5)             # 27.5   (not 27.500000)
    print(6.0)              # 6.0    (integral float keeps its .0, I#384)
    print(-4.5)             # -4.5
    print(True)             # True   (not -1)
    print(False)            # False
    print(42)               # 42
    print(-7)               # -7
    print("hello")          # hello
    print(1.5, True, 42)    # 1.5 True 42   (space-separated, each via str())
