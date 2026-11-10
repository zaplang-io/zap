# Golden fixture: concrete values stored in an interface-typed list element
# (#829, I#84 follow-up, D-0016). A `list[Shape]` element is a one-word cell, so
# each concrete conformer is widened into the interface's `{data, itable}` fat
# pointer and that fat pointer is boxed onto the heap for storage — the stored
# word is a stable heap pointer, not a dangling stack address. Reading an element
# back and calling a method dispatches dynamically through the itable. Two
# distinct implementors (Tri, Quad) at one call site prove the dispatch follows
# the concrete value behind each stored interface value.

interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

struct Quad:
    def sides(self) -> int:
        return 4

def total_sides(list[Shape] shapes) -> int:
    int acc = 0
    for s in shapes:
        acc = acc + s.sides()
    return acc

def main():
    # A heterogeneous polymorphic list built from a literal.
    list[Shape] shapes = [Tri(), Quad()]
    for s in shapes:
        print(s.sides())  # 3 then 4
    # Read an element by index and dispatch.
    print(shapes[0].sides())  # 3
    print(shapes[1].sides())  # 4
    # Pass the list to a function taking `list[Shape]` and dispatch each element.
    print(total_sides(shapes))  # 7
