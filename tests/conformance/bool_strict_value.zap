# claim: BOOL-2
# SPEC §5 (D-0022): `and`/`or` yield a STRICT bool (the surviving operand's
# truthiness), NOT the operand itself. This is a DELIBERATE divergence from
# Python, where `a and b` / `a or b` return one of the operands (Python would
# print 0 / 3 / 0 / 7 here). Rendered through str() so the bool is visible.
def main():
    print(str(1 and 0))
    print(str(2 and 3))
    print(str(0 or 0))
    print(str(0 or 7))
