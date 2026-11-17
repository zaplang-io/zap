# Golden fixture: str.translate / format_map / expandtabs (ticket #1034,
# D-0214). The remaining thin-str methods. Covers tab expansion, the
# character-map translate (replace + delete + no-match), format_map field
# substitution with literal braces, and the catchable KeyError / ValueError the
# malformed cases raise.
def main():
    # expandtabs: a tab advances to the next multiple of tabsize; 0 removes tabs.
    print("a\tb".expandtabs(4))  # a   b
    print("ab\tc".expandtabs(4))  # ab  c
    print(len("a\tb\tc".expandtabs()))  # 17  (default tabsize 8)
    print("x\ty".expandtabs(0))  # xy

    # translate: 1-char keys map to replacements; an empty value deletes.
    auto t = {"a": "A", "b": "", "c": "CC"}
    print("abcd".translate(t))  # ACCd
    print("zzz".translate(t))  # zzz  (no keys match)

    # format_map: {name} fields filled from the mapping; {{ }} are literal.
    auto m = {"name": "zap", "n": "42"}
    print("{name}-{n}".format_map(m))  # zap-42
    print("{{literal}}".format_map(m))  # {literal}

    # a missing field raises a catchable KeyError; a stray brace a ValueError.
    try:
        print("{missing}".format_map(m))
    except KeyError:
        print("key-error-caught")
    try:
        print("a}b".format_map(m))
    except ValueError:
        print("value-error-caught")
