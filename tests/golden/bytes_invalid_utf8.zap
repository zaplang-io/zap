# Golden fixture: str(bytes) raises a clear, catchable error on invalid UTF-8
# (ticket #337). 0xff is never a valid UTF-8 lead byte, so the decode fails.
def main():
    print("before")
    auto bad = bytes([255, 254])
    print(str(bad))
    print("after")
