# claim: EXC-4
# SPEC §4.3/§10 (D-0007, D-0014): `None` is the null value of a reference type,
# and member access on `None` throws a *catchable* built-in fault. Reading a
# field of a `None` struct receiver raises through the exception machinery
# (`zap_panic`), so a `try`/`except` catches it like an explicit `raise` and
# execution continues past the block (EXC-4).
struct Point:
    int64 x
    int64 y

def main():
    Point p = None
    try:
        print("body")
        auto v = p.x
        print(v)
    except Exception as e:
        print("caught")
    print("after")
