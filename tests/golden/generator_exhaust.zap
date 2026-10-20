# Golden fixture: straight-line `yield`s and exhaustion (I#67). `pair()` yields
# 10, 20, 30 then falls off the end; the `for` loop stops (the comma-ok
# `__next__` reports not-ok) and control continues to the final print.
def pair():
    yield 10
    yield 20
    yield 30

def main():
    for x in pair():
        print(x)
    print(999)
