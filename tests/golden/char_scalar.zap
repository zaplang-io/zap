# Golden fixture: a `char` scalar renders Python-style everywhere (#709).
# `char` is a distinct IR type (`Ty::Char`) — physically an i32 codepoint, but
# told apart from `int32` at the value layer — so a char from `s[i]`, `chr()`,
# `char()`, or an f-string interpolation renders as its one-codepoint string,
# both through `print` (which now routes `char` through the value renderer) and
# through `str()` / f-strings. `str`/f-string use `str` mode (unquoted); only a
# nested position (a struct field or collection element) quotes it `'c'`.
def main():
    auto s = "Hello"
    print(s[0])  # H   print(char) renders the character, not its codepoint
    print(str(s[1]))  # e
    print(f"{s[2]}")  # l
    print(chr(65))  # A
    print(char(66))  # B
    print(str(chr(97)))  # a
    # A char read by negative index is still a char.
    print(s[-1])  # o
