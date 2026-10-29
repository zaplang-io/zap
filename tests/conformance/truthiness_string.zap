# claim: TRUTH-2
# SPEC §5 / §9 (#385, D-0022): an empty string is falsy and a non-empty string
# is truthy, matching Python. (A `str` is a non-null heap pointer, so the
# falsiness is a length check, not a null check.)
def main():
    if not "":
        print("empty-falsy")
    if "x":
        print("nonempty-truthy")
    auto s = ""
    if not s:
        print("var-empty-falsy")
    auto t = "hi"
    if t:
        print("var-nonempty-truthy")
