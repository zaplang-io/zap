# Golden fixture: calling a refined-SUPERTYPE method on an interface value
# (#830, D-0139). `interface Ranked: Named` inherits `name` from `Named`. Before
# #830, `r.rank()` (own method) worked but `r.name()` (inherited) was ZAP6001
# "interface 'Ranked' has no method 'name'" at lowering, even though conformance
# across the refinement is accepted. The dispatch table now carries the
# supertype's methods in a stable slot order, so both the own and the inherited
# method dispatch correctly — and two implementors prove it is dynamic.

interface Named:
    def name(self) -> int

interface Ranked: Named
    def rank(self) -> int

struct Color:
    def name(self) -> int:
        return 1
    def rank(self) -> int:
        return 9

struct Grade:
    def name(self) -> int:
        return 2
    def rank(self) -> int:
        return 8

def describe(Ranked r) -> int:
    # Both the inherited and the own method, on a supertype-refined parameter.
    return r.name() * 10 + r.rank()

def main():
    Ranked r = Color()
    print(r.rank())  # 9   (own)
    print(r.name())  # 1   (inherited from Named)
    r = Grade()
    print(r.rank())  # 8   (dispatch follows the new concrete value)
    print(r.name())  # 2
    print(describe(Color()))  # 19
    print(describe(Grade()))  # 28
