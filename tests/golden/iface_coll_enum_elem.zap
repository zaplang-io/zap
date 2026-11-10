# Golden fixture (#831, ties to #731/#828): a SINGLE enum conformer — tier-3
# (data-carrying) and tier-1 (unboxed-integer) — at a collection VALUE position
# type-checks and dispatches. A tier-3 enum boxes its tagged-struct pointer into
# the interface fat pointer; a tier-1 enum boxes its scalar tag into a heap cell
# (the box's per-method thunks resolve the itable). Before #831 the single-enum
# list/dict literal was rejected by sema.

interface Ranked:
    def rank(self) -> int

enum Expr:
    Lit(int v)
    Add(int l, int r)

    def rank(self) -> int:
        return 1

enum Color:
    Red
    Green
    Blue

    def rank(self) -> int:
        return 7

def main():
    # Tier-3 enum conformer as a single list element.
    list[Ranked] xs = [Expr.Lit(v=5)]
    print(xs[0].rank())          # 1

    # Tier-1 enum conformer as a single list element.
    list[Ranked] ys = [Color.Green]
    print(ys[0].rank())          # 7

    # Tier-1 enum conformer as a dict value.
    dict[int, Ranked] d = {1: Color.Red}
    print(d[1].rank())           # 7
