# Golden fixture: a generator whose live local is a heap pointer (`str`) that
# persists across `yield`s (I#67). The state struct therefore has a pointer
# field, so this exercises precise/conservative GC tracing of generator state
# under both collectors. `joiner(4)` yields "x", "xx", "xxx", "xxxx"; the sum of
# their lengths is 1+2+3+4 = 10.
def joiner(int64 n):
    auto s = "x"
    auto i = 0
    while i < n:
        yield s
        s = s + "x"
        i = i + 1

def main():
    auto total = 0
    for w in joiner(4):
        total = total + len(w)
    print(total)
