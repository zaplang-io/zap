# diff: match
# Integer literal forms evaluate to the same runtime value as CPython (epic #374,
# walled garden): hex `0x`, octal `0o`, binary `0b`, and `_` digit grouping
# (including inside a radix literal). All are plain int64 here, well within range.
def main():
    print(0xFF)
    print(0o17)
    print(0b101)
    print(1_000)
    print(1_000_000)
    print(0xff)
    print(0b1111_0000)
    print(0o755)
    print(0x1F4)
    print(0xFF_FF)
