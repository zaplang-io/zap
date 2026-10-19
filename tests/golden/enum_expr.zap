# Golden fixture: a recursive enum (an AST) — proves the pointer-on-recursion
# representation (D-0046). `Add`/`Mul` carry `Expr` children; because a variant
# field whose type is the enum itself would make the value infinitely large, the
# compiler stores it as a GC pointer to a heap `Expr`, which `match` follows.
enum Expr:
    Num(int64 v)
    Add(Expr l, Expr r)
    Mul(Expr l, Expr r)

def eval(Expr e) -> int64:
    match e:
        case Expr.Num(v):
            return v
        case Expr.Add(l, r):
            return eval(l) + eval(r)
        case Expr.Mul(l, r):
            return eval(l) * eval(r)

def main():
    # 2 + (3 * 4) = 14
    auto e = Expr.Add(Expr.Num(2), Expr.Mul(Expr.Num(3), Expr.Num(4)))
    print(eval(e))
