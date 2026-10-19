# Golden fixture: operator overloading for user types (ticket #52, D-0047).
# A struct defines the arithmetic/equality/ordering dunders by hand; `+`, `==`,
# `!=`, `<`, `<=`, `>` desugar to those methods (the four relational operators
# derive from a single `__cmp__`, `!=` from `__eq__`). Augmented `+=` derives
# from `__add__`.
struct Vec2:
    int64 x
    int64 y

    def __add__(self, Vec2 other) -> Vec2:
        return Vec2(x=self.x + other.x, y=self.y + other.y)

    def __eq__(self, Vec2 other) -> bool:
        return self.x == other.x and self.y == other.y

    def __cmp__(self, Vec2 other) -> int64:
        if self.x < other.x:
            return -1
        if self.x > other.x:
            return 1
        if self.y < other.y:
            return -1
        if self.y > other.y:
            return 1
        return 0

def main():
    auto a = Vec2(x=1, y=2)
    auto b = Vec2(x=3, y=4)
    auto c = a + b
    print(c.x)
    print(c.y)
    if a == a:
        print("a == a")
    if a != b:
        print("a != b")
    if a < b:
        print("a < b")
    if a <= a:
        print("a <= a")
    if b > a:
        print("b > a")
    # Augmented assignment derives from `__add__`.
    c += a
    print(c.x)
    print(c.y)
