# Golden fixture: `append` a concrete value to an interface-typed list (#829).
# The empty-then-append form is the second repro in the issue: a `list[Shape]`
# starts empty and each `append(concrete)` widens the value into the fat pointer
# and heap-boxes it, exactly like the list-literal path. Appending two distinct
# implementors and dispatching proves the stored elements keep their concrete
# identity.

interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

struct Pent:
    def sides(self) -> int:
        return 5

def main():
    list[Shape] shapes = []
    shapes.append(Tri())
    shapes.append(Pent())
    shapes.append(Tri())
    print(len(shapes))  # 3
    for s in shapes:
        print(s.sides())  # 3, 5, 3
