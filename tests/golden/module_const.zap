# Module-level `const` (D-0112, #658 Phase 1): persistent immutable state at
# module scope, initialized once before `main`, usable across functions —
# scalars and reference (heap) types alike.
const int64 LIMIT = 100
const list[int64] XS = [1, 2, 3]
const dict[str, int64] TABLE = {"a": 10, "b": 20}

def sum_xs() -> int64:
    auto s = 0
    for x in XS:
        s += x
    return s

def main():
    print(LIMIT)
    print(sum_xs())
    print(TABLE["a"] + TABLE["b"])
