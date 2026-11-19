# Golden fixture: a struct with mixed field kinds over a channel (#1100).
#
# The received value must recover the *whole* struct type, so every field reads
# back correctly — a scalar `int`, a reference `str`, and a nested reference
# `struct` field. This guards against a fix that recovers only the first field
# or special-cases a single field kind.
struct Point:
    int x
    int y

struct Rec:
    int n
    str label
    Point at

def main():
    auto c = channel[Rec](1)
    c <- Rec(7, "hi", Point(1, 2))
    auto r = <-c
    print(r.n)  # 7
    print(r.label)  # hi
    print(r.at.x)  # 1
    print(r.at.y)  # 2
