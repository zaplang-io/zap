# Golden fixture: list[str] — pointer (reference-typed) elements: literal,
# indexing, len, and for-in iteration printing each element.
def main():
    auto names = ["ada", "alan", "grace"]
    print(len(names))
    print(names[1])
    for n in names:
        print(n)
