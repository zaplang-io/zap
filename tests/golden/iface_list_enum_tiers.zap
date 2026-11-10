# Golden fixture: both enum tiers stored as interface-typed list elements
# (#829 x #731, D-0016). A tier-3 (data-carrying) enum boxes its tagged-struct
# pointer into the fat pointer; a tier-1 (unboxed-integer) enum boxes its scalar
# tag into a heap cell first. Both fat pointers are then heap-boxed for the list
# cell. A struct conformer shares the same list, so one `list[Ranked]` holds all
# three kinds and dispatches each dynamically — the full struct/enum parity the
# interface-value representation promises.

interface Ranked:
    def rank(self) -> int

enum Expr:
    Lit(int v)
    Add(int l, int r)

    def rank(self) -> int:
        match self:
            case Expr.Lit(v):
                return v
            case Expr.Add(l, r):
                return l + r

enum Color:
    Red
    Green
    Blue

    def rank(self) -> int:
        return 100

struct Fixed:
    int n

    def rank(self) -> int:
        return self.n

def main():
    # Heterogeneous: a tier-3 enum, a tier-1 enum, and a struct behind one
    # interface-typed list. (Distinct element types, so the list is built via
    # the interface element path.)
    list[Ranked] items = [Expr.Add(l=1, r=2), Color.Green]
    items.append(Fixed(n=7))
    items.append(Expr.Lit(v=42))
    for it in items:
        print(it.rank())  # 3, 100, 7, 42
