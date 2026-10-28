# ticket #284 (addresses #282 string<->int): str()/int() conversions and the
# reachable UTF-8 byte view `encode()` (a list[int] of byte values, for SigV4).
def main():
    print(str(42))
    print(str(-7))
    print(str(char(65)))
    print(str(True))
    print(str(3.5))
    print(int("123") + 1)
    print(int("  -42  "))
    auto bs = "ABC".encode()
    print(len(bs))
    print(bs[0])
    print(bs[1])
    print(bs[2])
