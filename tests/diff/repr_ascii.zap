# diff: match
# repr() of built-in scalars and ascii() escaping match CPython exactly (#1384):
# strings gain Python quotes with the same single/double selection, control
# chars use short escapes, and ascii() backslash-escapes every non-ASCII
# codepoint as \xNN (<= 0xff) / \uNNNN (<= 0xffff) / \UNNNNNNNN (astral).
# Container repr is deliberately left to the golden suite — the corpus does not
# depend on container repr formatting (unspecified dict/set order).
def main():
    print(repr("hi"))
    print(repr("it's"))
    print(repr("a\nb"))
    print(repr("tab\there"))
    print(repr(42))
    print(repr(-7))
    print(repr(True))
    print(repr(False))
    print(ascii("abc"))
    print(ascii("café"))
    print(ascii("日本"))
    print(ascii("😀"))
    print(ascii(255))
