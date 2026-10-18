# Golden fixture: set[str] — pointer (reference-typed) elements compared by
# value: literal with a duplicate, len, membership (`in`), and add (insert +
# already-present).
def main():
    auto langs = {"py", "rs", "py"}
    print(len(langs))
    if "rs" in langs:
        print(1)
    else:
        print(0)
    if "go" in langs:
        print(1)
    else:
        print(0)
    langs.add("go")
    langs.add("rs")
    print(len(langs))
