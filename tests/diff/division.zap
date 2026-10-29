# diff: match
# True division `/` (float result) and floor division `//` over values where Zap
# and CPython agree (D-0024). Non-integral quotients are rendered through str()
# (print's own float format differs from CPython's repr by design — see
# conversions.zap). INTEGRAL float results are NOT here: `str(6 / 2)` is a
# documented divergence (Zap drops the `.0`) — see float_str_integral.zap.
def main():
    print(str(7 / 2))
    print(str(1 / 4))
    print(str(5 / 2))
    print(str(10 / 4))
    print(str(1 / 8))
    print(7 // 2)
    print(6 // 2)
    print(8 // 3)
    print(100 // 10)
