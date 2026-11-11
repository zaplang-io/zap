# Golden fixture (#837, D-0145): string correctness across the one-object
# (flexible-array-member) layout. A `str` is now a single GC object whose bytes
# are contiguous with the header; this guards that every string operation still
# sees the right bytes/length. Codepoint semantics (D-0025, #284) are unchanged:
# `len`, indexing, slicing and iteration are by Unicode codepoint, not byte.
def main():
    # Construction by concatenation of multibyte literals.
    auto s = "héllo" + " " + "wörld"
    print(len(s))  # 11  (codepoints, not the 13-byte length)

    int64 n = 0
    for c in s:
        n = n + 1
    print(n)  # 11  (iteration is by codepoint)

    print(ord(s[1]))  # 233   ('é')
    print(ord(s[7]))  # 246   ('ö')
    print(s[0:5])  # héllo
    print(s[6:11])  # wörld

    print("wör" in s)  # True
    print("zzz" in s)  # False
    print("hé" + "llo" == "héllo")  # True

    # Empty string: valid, zero length, compares equal to the empty literal.
    auto e = ""
    print(len(e))  # 0
    print(e == "")  # True

    # bytes view shares the ZapStr repr; its length is the UTF-8 byte count.
    print(len(bytes(s)))  # 13

    # A larger string built by repeated concatenation stays correct end to end.
    auto t = ""
    for i in range(0, 100):
        t = t + "ab"
    print(len(t))  # 200
    print(t[198:200])  # ab

    # f-strings: value interpolation and a nested f-string (#498/#285).
    print(f"{s} has {len(s)}")  # héllo wörld has 11
    auto inner = f"{2 + 3}"
    print(f"[{inner}]")  # [5]
