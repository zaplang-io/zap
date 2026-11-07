# diff: match
# Hex and Unicode string escapes decode to the codepoint, matching CPython
# (#374, fixed by #781): `\xHH`, `\uHHHH`, and `\UHHHHHHHH`. (`\N{...}` named
# escapes are a separate, not-yet-supported form.)
def main():
    print("\x41")
    print("café")
    print("\U0001F600")
    print("\x61\x62\x63")
