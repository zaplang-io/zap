# Golden fixture (#831 + D-0139/#806): a SINGLE conformer at a collection
# element whose declared type is a REFINED interface. `Ranked: Named` requires
# both its own `rank` and the inherited `name`; a type providing both is
# admitted as a `list[Ranked]` element, and BOTH the own and the inherited
# method dispatch through the effective itable (the supertype slots are part of
# the fat pointer's method list, #830). The return position is exercised too.

interface Named:
    def name(self) -> int

interface Ranked: Named
    def rank(self) -> int

struct Color:
    def name(self) -> int:
        return 42
    def rank(self) -> int:
        return 9

def make() -> list[Ranked]:
    # A single conformer returned as `list[Ranked]` (return-position coercion).
    return [Color()]

def main():
    list[Ranked] xs = make()
    print(xs[0].rank())   # 9  (own method)
    print(xs[0].name())   # 42 (inherited supertype method)
