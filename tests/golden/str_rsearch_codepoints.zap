# Golden fixture: str.rfind / str.rindex report CODEPOINT indices (#1630,
# superseding the byte-offset form of D-0210). The reverse-search methods now
# use the same units as find/index, len, indexing and slicing, so a returned
# index is always a valid codepoint position: s[s.rfind(x)] is well-formed and
# s[s.rfind(x):] slices at the match. CPython is the oracle for every line.
def main():
    # -- a multibyte prefix before an ASCII needle ----------------------------
    str s = "éx"
    print(len(s))  # 2
    print(s.find("x"))  # 1
    print(s.index("x"))  # 1
    print(s.rfind("x"))  # 1 (was 2 — the byte offset — before the fix)
    print(s.rindex("x"))  # 1
    print(s.rfind(""))  # 2 (empty needle -> len, in codepoints)
    print(s.rindex(""))  # 2
    # the returned index is usable for indexing and slicing
    print(s[s.rfind("x")])  # x
    print(s[s.rfind("x"):])  # x

    # -- empty needle on an empty haystack ------------------------------------
    print("".rfind(""))  # 0
    print("".rindex(""))  # 0

    # -- repeated matches with multibyte codepoints interleaved ---------------
    str t = "éxéx"
    print(len(t))  # 4
    print(t.find("x"))  # 1 (first x)
    print(t.rfind("x"))  # 3 (last x, codepoint index — byte offset would be 5)
    print(t.rfind("é"))  # 2 (last é)
    print(t.rfind("éx"))  # 2 (start of the last "éx")
    print(t[t.rfind("éx"):])  # éx

    # -- overlapping needle: the last matching START wins ---------------------
    print("aaaa".rfind("aa"))  # 2

    # -- a multibyte needle whose bytes recur ---------------------------------
    str v = "ééé"
    print(len(v))  # 3
    print(v.rfind("é"))  # 2
    print(v.rfind("éé"))  # 1

    # -- misses: rfind -> -1 --------------------------------------------------
    print("éx".rfind("z"))  # -1
    print("éx".rfind("xx"))  # -1 (needle longer than the tail)
    print("".rfind("a"))  # -1
    print("é".rfind("éé"))  # -1 (needle longer than the haystack)

    # -- rindex faults on a miss with a catchable ValueError (#1566) ----------
    try:
        print("éx".rindex("z"))
    except ValueError:
        print("rindex-miss-caught")
    try:
        print("".rindex("a"))
    except ValueError:
        print("empty-rindex-miss-caught")
