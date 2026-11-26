# diff: match
# Epic #1638: str.count(""), expandtabs column counting and char-set strip are
# codepoint-based, so they agree with CPython over multibyte strings. (translate
# is not in the shared subset — Zap keys a dict[str,str] by 1-char string, while
# CPython keys by ordinal — so it is pinned in the golden suite, whose expected
# output is generated from the equivalent CPython program, rather than here.)
def main():
    print("éx".count(""))
    print("é".count(""))
    print("éxé".count("é"))
    print("ééé".count("éé"))
    print("é\tA".expandtabs(8))
    print(len("é\tA".expandtabs(8)))
    print("éé\tx".expandtabs(4))
    print(len("é\n\tx".expandtabs(4)))
    print("αβγ".strip("α"))
    print("αβγα".strip("α"))
    print("αβγ".lstrip("α"))
    print("αβγ".rstrip("γ"))
    print("éαéβ".strip("éα"))
    print("xβx".strip("xβ"))
