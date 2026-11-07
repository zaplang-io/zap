# diff: match
# f-string format specs match CPython (epic #374, walled garden): field width,
# left/right/center alignment, a custom fill char, zero-padding, and the integer
# type specifiers (hex/binary/octal). Values are a str and an int so no float
# rounding is involved.
def main():
    auto x = "hi"
    print(f"[{x:>5}]")
    print(f"[{x:<5}]")
    print(f"[{x:^6}]")
    print(f"[{x:*^6}]")
    auto n = 42
    print(f"[{n:5}]")
    print(f"[{n:<5}]")
    print(f"[{n:05}]")
    print(f"{n:x}")
    print(f"{n:b}")
    print(f"{n:o}")
