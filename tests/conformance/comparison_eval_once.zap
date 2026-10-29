# claim: CMP-2
# SPEC §5 (#386): in a chained comparison the middle operand is evaluated EXACTLY
# ONCE (not once per sub-comparison). `mid()` has a side effect (printing), so a
# single "mid" line before the branch result proves single evaluation.
def mid() -> int64:
    print("mid")
    return 2


def main():
    if 1 < mid() < 3:
        print("in-range")
    else:
        print("out-of-range")
