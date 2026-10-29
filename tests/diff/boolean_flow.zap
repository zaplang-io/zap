# diff: match
# `not`, and the precedence of mixed `and`/`or` (`a and b or c` == `(a and b) or
# c`), observed through branch selection so the strict-bool result value (D-0022)
# never shows — only control flow, which agrees with CPython.
def yn(bool b) -> str:
    if b:
        return "Y"
    return "N"


def main():
    print(yn(not False))
    print(yn(not True))
    print(yn(True and True))
    print(yn(True and False))
    print(yn(False or True))
    print(yn(False or False))
    print(yn(True and False or True))
    print(yn(True and True or False))
    print(yn(False or False or True))
    print(yn(True and True and False))
    print(yn(not 1 > 2))
    print(yn(3 > 2 and 2 > 1))
