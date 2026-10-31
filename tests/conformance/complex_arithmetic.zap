# claim: CPLX-1
# SPEC §4.1 (D-0019): imaginary literals and arithmetic operate on a complex
# pair; rendering uses `(real±imagj)` and equality compares both components.
def main():
    auto z = complex(1.0, 2.0)
    print(str(z + 3j))
    print(str(z * 2j))
    print(str(z / complex(1.0, -1.0)))
    print(str(z ** 2))
    print(str(z == complex(1.0, 2.0)))
