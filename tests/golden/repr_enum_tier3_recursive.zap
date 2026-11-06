# Golden fixture: a recursive tier-3 enum renders recursively (#706, D-0124).
# A variant whose payload field is the enum itself (`Add(Expr l, Expr r)`)
# lowers the field as a pointer child; the renderer recurses by *calling* the
# same memoized per-enum renderer through that child, so an arbitrarily deep
# tree renders fully, terminating at compile time (one renderer per enum).
enum Expr:
    Lit(int64 v)
    Neg(Expr e)
    Add(Expr l, Expr r)


def main():
    # Lit(1) + Lit(2)
    print(Expr.Add(Expr.Lit(1), Expr.Lit(2)))  # Expr.Add(l=Expr.Lit(v=1), r=Expr.Lit(v=2))
    # (Lit(1) + Lit(2)) + Lit(3): left subtree is itself an Add.
    auto nested = Expr.Add(Expr.Add(Expr.Lit(1), Expr.Lit(2)), Expr.Lit(3))
    print(nested)  # Expr.Add(l=Expr.Add(l=Expr.Lit(v=1), r=Expr.Lit(v=2)), r=Expr.Lit(v=3))
    # Single-field recursive variant, nested twice.
    print(Expr.Neg(Expr.Neg(Expr.Lit(5))))  # Expr.Neg(e=Expr.Neg(e=Expr.Lit(v=5)))
    print(f"e={Expr.Lit(42)}")  # e=Expr.Lit(v=42)
