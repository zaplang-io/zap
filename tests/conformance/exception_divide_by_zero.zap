# claim: EXC-3
# SPEC §10 (D-0007): integer divide-by-zero is a *catchable* built-in fault. `//`
# and `%` by zero raise through the exception machinery (`zap_panic`), not a
# process abort, so a `try`/`except` around them catches the fault exactly like
# an explicit `raise` and execution continues past the block (#383-style routing,
# EXC-3). The fault is an `invoke` to the handler, not a plain `call`.
def main():
    int64 z = 0
    try:
        print("body")
        auto q = 1 // z
        print(q)
    except Exception as e:
        print("caught div")
    try:
        auto r = 1 % z
        print(r)
    except Exception as e:
        print("caught mod")
    print("after")
