# Golden fixture: these str methods are CODEPOINT-based, matching find/index,
# len, indexing and slicing — not UTF-8 byte-based. Epic #1638 (the post-#1630
# audit): count("") (#1634), expandtabs column counting (#1635), char-set strip
# (#1636) and translate keys (#1637). CPython is the oracle for every line; the
# .expected file is generated from the equivalent CPython program.
def main():
    # -- count: empty needle -> len(s) + 1 in CODEPOINTS (#1634) --------------
    print("éx".count(""))  # 3
    print("é".count(""))  # 2
    print("".count(""))  # 1
    print("abc".count(""))  # 4 (ASCII unchanged)
    # a non-empty needle's occurrence count is codepoint-independent
    print("éxé".count("é"))  # 2
    print("ééé".count("éé"))  # 1 (non-overlapping)
    print("éx".count("z"))  # 0

    # -- expandtabs: one column per codepoint, not per byte (#1635) -----------
    print("é\tA".expandtabs(8))  # é + 7 spaces + A
    print(len("é\tA".expandtabs(8)))  # 9
    print("éé\tx".expandtabs(4))  # éé + 2 spaces + x
    print(len("a\tb\tc".expandtabs()))  # 17 (ASCII, default tabsize 8)
    print(len("é\n\tx".expandtabs(4)))  # 7 (newline resets the column)

    # -- char-set strip: whole codepoints, result always valid UTF-8 (#1636) --
    print("éxé".strip("é"))  # x
    print("αβγ".strip("α"))  # βγ
    print("αβγα".strip("α"))  # βγ
    print("αβγ".lstrip("α"))  # βγ
    print("αβγ".rstrip("γ"))  # αβ
    print("éαéβ".strip("éα"))  # β
    print("xβx".strip("xβ"))  # (empty line)
    print(len("xβx".strip("xβ")))  # 0
    print("xxhixx".strip("x"))  # hi (ASCII unchanged)

    # -- translate: whole-codepoint keys map / delete / pass through (#1637) ---
    auto t1 = {"é": "e"}
    print("café".translate(t1))  # cafe
    auto t2 = {"c": "k"}
    print("café".translate(t2))  # kafé (ASCII key still works)
    auto t3 = {"β": ""}
    print("αβγ".translate(t3))  # αγ (multibyte delete)
    auto t4 = {"β": "XY"}
    print("αβγ".translate(t4))  # αXYγ (remap to a multibyte-free value)
    auto t5 = {"z": "Z"}
    print("αβγ".translate(t5))  # αβγ (no key matches -> unchanged)
    auto t6 = {"→": "-"}
    print("a→b".translate(t6))  # a-b (U+2192 key)
