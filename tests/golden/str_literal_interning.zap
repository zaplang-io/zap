# Golden fixture (#837, D-0145): string-literal interning. Each distinct literal
# lowers to ONE never-freed constant-global `ZapStr`, so two uses of the same
# literal are the SAME object (CPython-like `is` identity), while a string built
# at runtime (concat / f-string with a field) is a distinct heap object that is
# `==`-equal but not `is`-equal. Interning must also be non-destructive: concat
# reads the constant and allocates a fresh result, never mutating or freeing the
# shared literal, so the literal is intact after being used in a concat.
def main():
    auto a1 = "intern_me"
    auto a2 = "intern_me"
    print(a1 is a2)  # True   (same interned literal object)

    auto built = "intern" + "_me"
    print(a1 == built)  # True   (value equality)
    print(a1 is built)  # False  (built is a fresh heap string)

    # Using the literal in a concat does not corrupt or free it.
    auto combo = a1 + "!"
    print(combo)  # intern_me!
    print(a1)  # intern_me   (unchanged)
    print(len(a1))  # 9

    # The same literal reused many times stays one object.
    bool allsame = True
    for i in range(0, 1000):
        if not "intern_me" is a1:
            allsame = False
    print(allsame)  # True
