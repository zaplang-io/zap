# Golden fixture: a tier-3 (data-carrying) enum conforms to an interface and is
# usable as an interface value (#731, D-0016 extended to enums). This is the
# exact issue repro: `Node n = Expr.Add(...)` was `error[ZAP4001]` because
# conformance was struct-only. A tier-3 enum lowers to a tagged struct, so it
# boxes that pointer into the interface's `{data, itable}` fat pointer and
# dispatches through the itable exactly like a struct — its method symbols
# (`Expr__rank`) are the itable slots. The `match self` body reads the payload,
# proving dispatch runs the real method on the concrete value, not a stub.

interface Node:
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

# Round-trip: pass the enum as an interface-typed parameter and dispatch.
def rank_of(Node n) -> int:
    return n.rank()

def main():
    # Dispatch through an interface-typed binding.
    Node n = Expr.Add(l=1, r=2)
    print(n.rank())  # 3  (1 + 2)
    Node m = Expr.Lit(v=5)
    print(m.rank())  # 5

    # Re-bind the SAME interface variable to a different variant: dispatch
    # follows the concrete value behind the fat pointer.
    n = Expr.Lit(v=42)
    print(n.rank())  # 42

    # Round-trip through an interface-typed parameter.
    print(rank_of(Expr.Add(l=10, r=20)))  # 30
    print(rank_of(Expr.Lit(v=7)))  # 7
