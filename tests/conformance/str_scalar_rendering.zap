# claim: STR-1
# SPEC §9 (#282/#285/#384, D-0026): `str(x)` renders scalars deterministically —
# an int as its decimal form, a bool as `True`/`False`, and a float ALWAYS with a
# decimal point, so an integral float keeps its `.0` (`str(6.0)` and `str(6 / 2)`
# are both `"6.0"`, after #384).
def main():
    print(str(42))
    print(str(-7))
    print(str(3.14))
    print(str(6.0))
    print(str(6 / 2))
    print(str(True))
    print(str(False))
