# Golden fixture: bytes<->hex encode/decode (ticket #337). Lowercase hex, even
# length, round-trips through decode and back.
def main():
    print(hex.encode(b"foobar"))  # 666f6f626172
    print(hex.encode(bytes([0, 255, 16])))  # 00ff10
    print(hex.encode(hex.decode("deadbeef")))  # deadbeef
    print(str(hex.decode("68656c6c6f")))  # hello
    auto d = hex.decode("00ff10")
    print(d[1])  # 255
    print(len(d))  # 3
