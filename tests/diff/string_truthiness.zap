# diff: match
# String truthiness (#385, D-0022): an empty string is falsy and a non-empty
# string is truthy, matching CPython. Zap used to treat every string as truthy
# regardless of length (a `str` is a non-null heap pointer), so `if "":` took
# the then-branch and `not ""` was False; the `truthy` lowering now emits a byte
# length check (len 0 -> false). Numeric falsiness (0, 0.0) and non-empty-string
# truthiness already matched (see truthiness.zap).
def tag(bool b) -> str:
    if b:
        return "T"
    return "F"


def main():
    if "":
        print("nonempty-branch")
    else:
        print("empty-branch")
    print(tag(not ""))
