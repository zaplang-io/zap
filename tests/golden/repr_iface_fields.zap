# Golden fixture: interface-value rendering recurses into every field type
# (#707, D-0124). A struct behind an interface renders through the itable's
# renderer slot, and that renderer renders each field via the one Display
# protocol — so a tier-1 enum field, a tier-3 data-carrying enum field (#706's
# structural renderer, reached THROUGH the interface — the key integration
# case), a tuple field, a quoted `str` field, and a `list` field all render
# exactly as they would on a concrete value. (A tier-1/tier-3 enum cannot itself
# sit directly behind an interface — only a struct conforms today, D-0016 — so
# the enum renderers are exercised here as struct fields of the interface value.)

interface Describable:
    def tag(self) -> str

enum Color:
    Red
    Green

enum Shape:
    Dot
    Box(int w, int h)

struct Widget:
    str name
    Color c
    Shape s
    (int, str) pair
    list[int] xs

    def tag(self) -> str:
        return self.name

def main():
    Describable d = Widget(
        name="w1",
        c=Color.Green,
        s=Shape.Box(w=3, h=4),
        pair=(7, "hi"),
        xs=[1, 2],
    )
    print(d)  # Widget(name='w1', c=Color.Green, s=Shape.Box(w=3, h=4), pair=(7, 'hi'), xs=[1, 2])
    print(str(d))  # str() matches print
    print(f"{d}")  # f-string matches print
