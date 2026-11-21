# Golden: operator capabilities as generic bounds (#1199).
#
# The prelude declares the operator-capability interfaces (D-0047/#1180/#1198):
# the forward set (`Add[R, Out]`, `Mul[R, Out]`, `Ord[R]`, …) and the reflected
# set (`RMul[L, Out]`, `RAdd[L, Out]`, …). A generic can require one as a bound
# and use the operator in its body, typed through the bound. Conformance is
# structural and enforced per instantiation by the monomorphizer (static
# dispatch, no fat pointer), so this adds no runtime mechanism — the concrete
# `a OP b` dispatch is exactly the one that already shipped (#1192/#1194/#1202).

value struct Vec2:
    f64 x
    f64 y

    # Scalar-right multiply: `k * v` dispatches to `v.__rmul__(k)` (#1194) and
    # names Vec2 a conformer of `RMul[f64, Vec2]`.
    def __rmul__(self, f64 k) -> Vec2:
        return Vec2(x=self.x * k, y=self.y * k)

    # Vector add: `a + b`; names Vec2 a conformer of `Add[Vec2, Vec2]`.
    def __add__(self, Vec2 o) -> Vec2:
        return Vec2(x=self.x + o.x, y=self.y + o.y)

value struct Money:
    int64 cents

    # Ordering via the single-method `__cmp__` (D-0047): names Money a conformer
    # of `Ord[Money]`, so `<` works on a `T: Ord[Money]` bound.
    def __cmp__(self, Money o) -> int64:
        return self.cents - o.cents

# Reflected bound — the motivating case: scale every element by an f64 through
# `RMul[f64, T]`. The comprehension builds a `list[T]` because `Out := T`.
def scale_all[T: RMul[f64, T]](list[T] xs, f64 k) -> list[T]:
    return [k * x for x in xs]

# Forward bound: sum two values through `Add[T, T]` (`Out := T`).
def add2[T: Add[T, T]](T a, T b) -> T:
    return a + b

# Comparison bound: the relational result is `bool`.
def smaller[T: Ord[T]](T a, T b) -> bool:
    return a < b

def main():
    list[Vec2] vs = [Vec2(x=1.0, y=2.0), Vec2(x=3.0, y=4.0)]
    list[Vec2] scaled = scale_all(vs, 2.0)
    for v in scaled:
        print(v.x)
        print(v.y)
    Vec2 s = add2(Vec2(x=1.0, y=1.0), Vec2(x=2.0, y=3.0))
    print(s.x)
    print(s.y)
    print(smaller(Money(cents=1), Money(cents=2)))
    print(smaller(Money(cents=5), Money(cents=5)))
