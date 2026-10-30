# claim: OP-IN-2
# SPEC §5 (#399): `x in <list>` tests list membership — true when some element
# equals `x`, using the same element equality as `==`. `not in` is written
# `not (x in xs)` (Zap has no `not in` operator). Mirrors dict-key and substring
# membership (OP-IN-1); matches CPython.
def main():
    list[int64] xs = [5, 6, 7, 8]
    if 7 in xs:
        print(1)
    else:
        print(0)
    if 99 in xs:
        print(1)
    else:
        print(0)
    if not 99 in xs:
        print(1)
    else:
        print(0)
    list[str] langs = ["py", "rs", "zap"]
    if "rs" in langs:
        print(1)
    else:
        print(0)
    if "go" in langs:
        print(1)
    else:
        print(0)
