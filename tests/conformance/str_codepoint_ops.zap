# claim: STR-2
# SPEC §9.1 (#284, D-0025): string indexing/slicing/iteration are by Unicode
# codepoint. `s[i]` yields a `char` (ord() gives its codepoint), a slice yields a
# `str` with Python bounds/negatives/step (`s[::-1]` reverses), a negative index
# counts from the end, and `in` tests substring membership.
def main():
    auto s = "Hello, World"
    print(ord(s[0]))
    print(s[7:])
    print(s[::-1])
    print(str(s[-1]))
    if "World" in s:
        print("has-world")
    if "xyz" in s:
        print("has-xyz")
    else:
        print("no-xyz")
