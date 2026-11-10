# Golden fixture (#831): a SINGLE concrete conformer at a collection VALUE
# position — a list element, a dict value, and a nested-list element — now
# type-checks and runs end-to-end. Before #831 sema rejected these (ZAP4001)
# because a single conformer keeps its concrete element type (`list[Tri]`,
# `dict[int, Tri]`) rather than widening to `Unknown`; only a heterogeneous
# literal slipped through. The IR boxing (#829/#832) was already in place, so
# admitting the coercion makes each element dispatch dynamically through the
# interface itable.

interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

struct Quad:
    def sides(self) -> int:
        return 4

def first_sides(list[Shape] shapes) -> int:
    return shapes[0].sides()

def main():
    # Single conformer as a list element.
    list[Shape] xs = [Tri()]
    print(xs[0].sides())        # 3
    print(first_sides(xs))      # 3

    # Single conformer as a dict VALUE; the key stays an exact `int`.
    dict[int, Shape] d = {1: Tri()}
    print(d[1].sides())         # 3

    # Distinct conformer values in one dict (values widen to Unknown, then are
    # admitted by real conformance).
    dict[int, Shape] e = {1: Tri(), 2: Quad()}
    print(e[1].sides())         # 3
    print(e[2].sides())         # 4

    # Nested list of interface: the descent recurses to the innermost element.
    list[list[Shape]] nested = [[Tri()]]
    print(nested[0][0].sides()) # 3
