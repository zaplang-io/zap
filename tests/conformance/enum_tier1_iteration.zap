# claim: ENUM-1
# SPEC §4.6 (D-0018): tier-1 enums keep their identity, canonicalize aliases,
# expose the canonical member through `.name`, and iterate in declaration order.
enum Color:
    RED = 2
    ALSO_RED = 2
    GREEN = 10
    BLUE

enum Empty:
    pass

def main():
    auto alias = Color.ALSO_RED
    print(alias.name)
    for color in Color:
        print(color.name)
        print(int64(color))
    for color in Empty:
        print(color.name)
    print("done")
