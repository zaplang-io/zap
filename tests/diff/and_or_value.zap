# diff: diverge D-0022: boolean and/or yield a STRICT bool (the surviving
# operand's truthiness), not the operand itself. CPython's `a and b` / `a or b`
# return one of the operands (so `1 or 2` is 1, `"" or "x"` is "x"); Zap always
# returns True/False. Short-circuit evaluation is the same (see short_circuit.zap)
# — only the result VALUE differs. Rendered via str() so the difference is
# visible: CPython prints 2 / 1 / x / 0; Zap prints True / True / True / False.
def main():
    print(str(0 or 2))
    print(str(1 or 2))
    print(str("" or "x"))
    print(str(0 and 5))
