# diff: match
# Non-ASCII string indexing and slicing is by Unicode codepoint (D-0025/D-0026,
# B6 fixed), matching CPython: a multibyte character is one index position, and
# slices / reverse / steps operate on codepoints, not UTF-8 bytes. Each result is
# a valid UTF-8 substring, so stdout is byte-identical to CPython.
def main():
    auto s = "héllo"
    print(s[1])
    print(s[-1])
    print(s[1:3])
    print(s[::-1])
    auto t = "日本語"
    print(t[0])
    print(t[1:])
    print(t[::-1])
    print(t[0:2])
    print(len(t))
