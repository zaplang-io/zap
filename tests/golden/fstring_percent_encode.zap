# ticket #285: byte -> %XX percent-encoding, the SigV4 canonicalization use
# case that motivated f-string codegen + `{b:02X}` hex specs.
def main():
    auto bytes = [10, 47, 255, 32, 126]
    auto out = ""
    for b in bytes:
        out = out + f"%{b:02X}"
    print(out)
