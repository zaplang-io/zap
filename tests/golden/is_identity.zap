# Golden fixture: `is` / `is not` identity operators (#611, D18).
#
# `is` tests reference identity (same allocation), distinct from `==` (value /
# `__eq__` equality). `x is None` / `x is not None` is the empty-variant / null
# check, consistent with `.is_none()` and `match case Optional.None`. `is`/`is
# not` chain like the other comparison operators (I#386).

# A reference struct with value equality (`__eq__`), so the fixture can show
# that two DISTINCT allocations with equal fields are `==`-equal but NOT
# `is`-equal — the defining difference between the two operators.
struct Box:
    int64 v

    def __eq__(self, Box other) -> bool:
        return self.v == other.v

def main():
    # --- reference-struct identity: same allocation vs distinct ---
    Box a = Box(v=1)
    Box b = a  # same allocation as a
    Box c = Box(v=1)  # distinct allocation, equal field
    print(str(a is b))  # True  (same object)
    print(str(a is c))  # False (distinct objects)
    print(str(a is not c))  # True
    print(str(a == c))  # True  (value equality via __eq__)
    print(str(a is a))  # True

    # --- reference struct nullability: `is None` is the null check ---
    Box n = None
    print(str(n is None))  # True
    print(str(n is not None))  # False
    print(str(a is None))  # False
    print(str(a is not None))  # True

    # --- Optional[int64] (tagged layout): empty-variant check ---
    Optional[int64] some = Optional.Some(5)
    Optional[int64] non = None
    print(str(some is None))  # False
    print(str(some is not None))  # True
    print(str(non is None))  # True
    print(str(non is not None))  # False

    # --- Optional[str] (niche layout): empty-variant check ---
    Optional[str] s = Optional.Some("hi")
    Optional[str] t = None
    print(str(s is not None))  # True
    print(str(t is None))  # True

    # --- `None is None` folds to a constant ---
    print(str(None is None))  # True
    print(str(None is not None))  # False

    # --- chained identity: `a is b is a` is (a is b) and (b is a) ---
    print(str(a is b is a))  # True  (all the same object)
    print(str(a is b is c))  # False (b is c is False)

    print("done")
