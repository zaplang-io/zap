# Golden fixture: multi-level interface refinement A: B: C (#830, D-0139). A
# value typed as the most-derived interface (C) can dispatch a method introduced
# at every level of the chain — the dispatch table folds in the transitive
# supertype methods in a stable order, so each level's method resolves to its own
# slot regardless of nesting depth.

interface A:
    def a(self) -> int

interface B: A
    def b(self) -> int

interface C: B
    def c(self) -> int

struct S:
    def a(self) -> int:
        return 1
    def b(self) -> int:
        return 20
    def c(self) -> int:
        return 300

struct T:
    def a(self) -> int:
        return 2
    def b(self) -> int:
        return 40
    def c(self) -> int:
        return 600

def main():
    C x = S()
    print(x.a())  # 1    (grandparent A)
    print(x.b())  # 20   (parent B)
    print(x.c())  # 300  (own C)
    print(x.a() + x.b() + x.c())  # 321
    # A second conformer dispatches each level on the same slots.
    x = T()
    print(x.a() + x.b() + x.c())  # 642
