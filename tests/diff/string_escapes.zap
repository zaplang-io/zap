# diff: match
# The common string escape sequences decode the same as CPython (epic #374,
# walled garden): `\t`, `\n`, `\r`, `\\`, `\"`, and `\'`. (The hex/Unicode
# escapes `\x`/`\u`/`\U` are a separate, diverging gap — see
# string_escapes_unsupported.zap and #781.)
def main():
    print("a\tb")
    print("line1\nline2")
    print("quote\"end")
    print("back\\slash")
    print("apos\'end")
    print("car\rriage")
