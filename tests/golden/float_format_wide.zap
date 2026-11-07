# Golden (epic #374, B5): a float format spec wider than the old fixed buffer is
# not truncated. `{1.0:.100f}` renders "1." followed by 100 fractional digits
# (all zeros, since 1.0 is exact) = 102 characters; before the dynamic-buffer fix
# it was cut off near 95.
def main():
    auto s = f"{1.0:.100f}"
    print(len(s))
