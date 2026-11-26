# Golden fixture: str.partition / rpartition (ticket #1029, D-0218). Each splits
# on the first / last occurrence of the separator and returns a (head, sep, tail)
# tuple (the process.run aggregate path). Covers a present separator, an absent
# one (partition -> (whole,"",""); rpartition -> ("","",whole)), and the
# catchable ValueError an empty separator raises.
def main():
    auto h, s, t = "a=b=c".partition("=")
    print(h)  # a
    print(s)  # =
    print(t)  # b=c

    auto h2, s2, t2 = "a=b=c".rpartition("=")
    print(h2)  # a=b
    print(s2)  # =
    print(t2)  # c

    auto h3, s3, t3 = "abc".partition("=")
    print(h3)  # abc
    print(len(s3))  # 0
    print(len(t3))  # 0

    auto h4, s4, t4 = "abc".rpartition("=")
    print(len(h4))  # 0
    print(len(s4))  # 0
    print(t4)  # abc

    try:
        auto a, b, c = "x".partition("")
        print(a)
    except ValueError:
        print("empty-sep-caught")

    # Non-ASCII regression (#1630): rpartition slices the raw UTF-8 buffer at
    # the LAST separator, so it must keep using the byte offset even though the
    # public rfind/rindex now report codepoint indices.
    auto h5, s5, t5 = "éxéx".rpartition("x")
    print(h5)  # éxé
    print(s5)  # x
    print(len(t5))  # 0
    auto h6, s6, t6 = "é=é=é".rpartition("=")
    print(h6)  # é=é
    print(s6)  # =
    print(t6)  # é
