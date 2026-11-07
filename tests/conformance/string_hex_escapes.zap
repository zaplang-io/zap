# claim: STRLIT-1
# SPEC §9 (#374, #781): `\x`/`\u`/`\U` escapes decode to the codepoint (UTF-8) in
# a str; in a bytes literal `\xHH` is the raw byte (and `\u`/`\U` are not special).
def main():
    print("\x41")
    print("café")
    print("\U0001F600")
    auto b = b"\xff\x00\x41"
    print(len(b))
    print(b[0])
    print(b[1])
    print(b[2])
