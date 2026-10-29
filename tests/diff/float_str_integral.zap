# diff: diverge BUG: str() of an integral-valued float drops the `.0`. CPython
# renders a float as a float — `str(6 / 2)` is "3.0" — but Zap renders it as
# "3", indistinguishable from the int. This makes round-tripping and
# float/int-sensitive output diverge. (Non-integral floats like 3.5 / 0.25 DO
# match — see division.zap.) A real bug to triage. CPython: 3.0, 6.0, 4.0, -2.0;
# Zap: 3, 6, 4, -2.
def main():
    print(str(6 / 2))
    print(str(3.0 * 2.0))
    print(str(2.5 + 1.5))
    print(str(-4.0))
