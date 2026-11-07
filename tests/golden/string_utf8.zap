# ticket #284: UTF-8 codepoint semantics (D-0025). Indexing, slicing, iteration
# and `len` are all by Unicode codepoint, not byte; `str(char)` re-encodes to
# UTF-8. `len("héllo €")` is 7 codepoints (not the 10-byte length); the
# subsequent iteration count confirms it (#374/B6).
def main():
    auto s = "héllo €"
    print(len(s))
    int64 n = 0
    for c in s:
        n = n + 1
    print(n)
    print(ord(s[1]))
    print(ord(s[6]))
    print(s[0:5])
    print(str(char(8364)))
