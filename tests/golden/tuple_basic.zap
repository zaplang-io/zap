# Golden fixture: tuple literal + element access by a compile-time-constant
# index (D-0049). `t[k]` yields that position's static type; a runtime index
# would be a compile error (a tuple is heterogeneous).
def main():
    auto t = (10, 20, 30)
    print(t[0])
    print(t[1])
    print(t[2])
