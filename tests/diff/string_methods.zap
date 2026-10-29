# diff: match
# Core str methods that mirror Python exactly (#284): case, strip, split/join,
# find/index, replace, startswith/endswith, and `in` for substring membership
# (Zap's `.contains()` is deliberately excluded — it has no Python equivalent).
def mark(bool b) -> int64:
    if b:
        return 1
    return 0


def main():
    print("Hello".upper())
    print("Hello".lower())
    print("  hi  ".strip())
    print("  hi".lstrip())
    print("hi  ".rstrip())
    auto parts = "a,b,c".split(",")
    print(len(parts))
    print(parts[0])
    print(parts[2])
    print("-".join(parts))
    print("Hello World".find("World"))
    print("Hello World".find("xyz"))
    print("Hello World".index("o"))
    print("aaa".replace("a", "bb"))
    print(mark("hello.py".startswith("hello")))
    print(mark("hello.py".endswith(".py")))
    print(mark("ell" in "hello"))
    print(mark("zz" in "hello"))
