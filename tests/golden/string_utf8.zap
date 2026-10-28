# ticket #284: UTF-8 codepoint semantics (D-0025). Indexing, slicing and
# iteration are by Unicode codepoint, not byte; `str(char)` re-encodes to UTF-8.
# `len` is the byte length here (it matches the codepoint count only for ASCII;
# a codepoint-based `len(str)` is deferred — see #284), so the codepoint count
# is shown via iteration.
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
