# Golden fixture: a nested `list[list[Shape]]` of interface elements (#829).
# Each inner list boxes its concrete elements into the interface fat pointer and
# heap-boxes them; the outer list stores each inner-list pointer as a nested
# element. Indexing two levels down and dispatching proves the boxing composes
# through nesting.

interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

struct Quad:
    def sides(self) -> int:
        return 4

def main():
    list[list[Shape]] grid = [[Tri(), Quad()], [Quad(), Tri()]]
    for row in grid:
        for s in row:
            print(s.sides())  # 3, 4, 4, 3
    # Two-level indexed read + dispatch.
    print(grid[0][1].sides())  # 4
    print(grid[1][0].sides())  # 4
