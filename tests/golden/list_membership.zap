# Golden fixture: list membership `in` (ticket #399) and `not in` (ticket #593)
# — a linear scan comparing each element to the needle for equality (same
# element equality as `==`). Covers a present element, an absent one, and the
# `not in` operator, over both an int64 list and a str list (pointer elements
# compared by value, not identity). `x not in xs` lowers identically to
# `not (x in xs)`.
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
    if 99 not in xs:
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
    if "go" not in langs:
        print(1)
    else:
        print(0)
