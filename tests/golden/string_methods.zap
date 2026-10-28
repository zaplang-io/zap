# ticket #284: the core str method surface — case, strip, split/join, find/index,
# replace, startswith/endswith/contains.
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
    print("foobar".replace("o", "0"))
    if "hello.py".startswith("hello"):
        print("sw")
    if "hello.py".endswith(".py"):
        print("ew")
    if "hello".contains("ell"):
        print("has")
