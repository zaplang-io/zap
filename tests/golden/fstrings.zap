# ticket #285 (D-0026): f-string interpolation and the format mini-language.
def main():
    auto n = 42
    auto x = 3.14159
    auto name = "zap"
    print(f"n={n}")
    print(f"{name} v{n}")
    print(f"{{braces}} and {n}")
    print(f"hex {n:x} oct {n:o} bin {n:b}")
    print(f"{n:02x}")
    print(f"{255:X}")
    print(f"{7:04d}")
    print(f"[{n:5}]")
    print(f"[{n:<5}]")
    print(f"[{n:^7}]")
    print(f"[{name:>6}]")
    print(f"[{name:<6}]")
    print(f"{x:.2f}")
    print(f"{x:10.3f}")
    print(f"{n:+d} {n:#x} {n:#o}")
    auto empty = f""
    print(f"[{empty}]")
