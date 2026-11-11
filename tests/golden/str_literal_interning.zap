# Golden fixture (#837, D-0145): string-literal interning is TRANSPARENT. Each
# distinct literal lowers to one never-freed constant-global `ZapStr` reused at
# every use, but `str` is a value type with no observable identity (`is` on `str`
# is a compile error, ZAP4073 / #611), so interning changes nothing semantically:
# value `==` holds, and — the real risk this guards — concat/format read the
# shared constant and allocate a fresh result, so they never mutate or free it.
def main():
    auto a1 = "intern_me"
    auto a2 = "intern_me"
    print(a1 == a2)  # True   (two uses of the same literal are value-equal)

    # A string built at runtime equals the literal by value.
    auto built = "intern" + "_me"
    print(a1 == built)  # True

    # Concatenation must NOT mutate or free the shared interned constant.
    auto combo = a1 + "!"
    print(combo)  # intern_me!
    print(a1)  # intern_me   (unchanged)
    print(len(a1))  # 9

    # Reuse the same literal in many concats; the constant stays intact each time
    # (the concat's prefix is still the literal, and the literal itself is intact).
    bool ok = True
    for i in range(0, 1000):
        auto t = "intern_me" + str(i)
        if t[0:9] != "intern_me":
            ok = False
        if "intern_me" != a1:
            ok = False
    print(ok)  # True
    print(a1)  # intern_me   (still intact after 1000 concats)
