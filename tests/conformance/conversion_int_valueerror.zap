# claim: CONV-1
# SPEC §9 (#456): int(s) on a malformed string raises a *catchable* `ValueError`
# (Python parity, #383-style routing), not an uncatchable process abort. A
# `try`/`except ValueError` around it recovers exactly like an explicit `raise`:
# the handler runs, binds a Python-style message, and execution continues past
# the block. Valid parses — surrounding ASCII whitespace and a leading sign —
# are unaffected.
def main():
    try:
        auto n = int("abc")
        print(str(n))
    except ValueError as e:
        print("caught")
        print(e)
    print("after")
    auto ok = int("  -42  ")
    print(str(ok))
