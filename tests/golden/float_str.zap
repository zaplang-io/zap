# Golden fixture: `str(float)` rendering (I#384). `str()` of a float always
# renders as a float, matching CPython's `str(float)`: an integral value keeps a
# trailing `.0` so it stays visibly distinct from an int, non-integral values
# render compactly, the sign is preserved, and the special values inf/-inf/nan
# are spelled as CPython spells them. (Note: `print(float)` uses a separate
# fixed `%f` rendering — see scalars.zap — this fixture is about `str`.)
import math

def main():
    print(str(6 / 2))  # 3.0   (integral float keeps .0)
    print(str(3.0 * 2.0))  # 6.0
    print(str(-4.0))  # -4.0  (negative integral)
    print(str(0.0))  # 0.0   (zero)
    print(str(100.0))  # 100.0
    print(str(3.5))  # 3.5   (non-integral unchanged)
    print(str(0.25))  # 0.25
    print(str(math.inf))  # inf
    print(str(-math.inf))  # -inf
    print(str(math.nan))  # nan
