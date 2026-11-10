# Golden fixture: a refined-supertype method on an interface value backed by an
# ENUM, both tiers (#830 x #731, D-0016/D-0139). `interface Ranked: Named`; a
# tier-3 and a tier-1 enum each conform (both `name` and `rank`) and dispatch the
# inherited `name()` through the supertype slot the itable laid out — the
# supertype slots work for enum conformers exactly as for structs.

interface Named:
    def name(self) -> int

interface Ranked: Named
    def rank(self) -> int

enum Expr:
    Lit(int v)
    Add(int l, int r)

    def name(self) -> int:
        return 1
    def rank(self) -> int:
        match self:
            case Expr.Lit(v):
                return v
            case Expr.Add(l, r):
                return l + r

enum Color:
    Red
    Green

    def name(self) -> int:
        return 2
    def rank(self) -> int:
        return 50

def main():
    # Tier-3 enum behind the refined interface.
    Ranked a = Expr.Add(l=4, r=5)
    print(a.rank())  # 9    (own)
    print(a.name())  # 1    (inherited)
    # Tier-1 enum behind the refined interface.
    Ranked b = Color.Green
    print(b.rank())  # 50   (own)
    print(b.name())  # 2    (inherited)
