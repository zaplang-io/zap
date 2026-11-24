# Regression for #1367: a bare `None` argument to an enum-variant constructor
# `Enum.Variant(None)` must lower to the field's canonical empty value, not bail
# at IR lowering with `ZAP6001`. The enum-variant constructor lowered its payload
# argument with a bare `self.expr(arg)` (no type context), unlike struct fields
# (#1075), so a bare `None` fell through to the generic "Phase 1 slice" bail even
# though sema typed it. Routing the argument through the shared `expr_coerced`
# chokepoint (`lower_none_expr` / `gen_opt_some`, the #1262/#1075 mechanism) fixes
# it, matching struct-constructor fields.
#
# Covers the primary valid case — a nested `Optional` (`Optional.Some(None)` for
# `Optional[Optional[T]]`, an outer-present / inner-absent value) with both a
# scalar inner payload and a pointer inner payload — and a user enum carrying an
# `Optional[T]` field constructed with a bare `None`. All reads are native
# (is_some/is_none/unwrap/unwrap_or + match), so the representation is observed.
# (A nullable-payload Optional is tagged, not niche-optimized, so `Some(None)` is
# distinct from `None`.)

enum E:
    V(Optional[int64] x)
    W(Optional[str] s)
    Nil


def describe_v(E e) -> int64:
    match e:
        case E.V(x):
            return x.unwrap_or(-5)
        case E.W(s):
            return -2
        case E.Nil:
            return -1


def describe_w(E e) -> str:
    match e:
        case E.W(s):
            if s.is_none():
                return "w-none"
            return s.unwrap()
        case E.V(x):
            return "v"
        case E.Nil:
            return "nil"


def main():
    # Nested Optional, scalar inner payload: outer Some wrapping an inner None.
    Optional[Optional[int64]] a = Optional.Some(None)
    print(a.is_some())  # True
    print(a.unwrap().is_none())  # True
    Optional[Optional[int64]] b = Optional.Some(Optional.Some(3))
    print(b.unwrap().unwrap())  # 3

    # Nested Optional, pointer inner payload.
    Optional[Optional[str]] c = Optional.Some(None)
    print(c.is_some())  # True
    print(c.unwrap().is_none())  # True
    Optional[Optional[str]] d = Optional.Some(Optional.Some("hi"))
    print(d.unwrap().unwrap())  # hi

    # Outer None control (not Some(None)).
    Optional[Optional[int64]] n = None
    print(n.is_none())  # True

    # User enum variant with a scalar-payload Optional field: bare None vs Some.
    print(describe_v(E.V(None)))  # -5
    print(describe_v(E.V(Optional.Some(7))))  # 7
    print(describe_v(E.Nil))  # -1

    # User enum variant with a pointer-payload Optional field.
    print(describe_w(E.W(None)))  # w-none
    print(describe_w(E.W(Optional.Some("hey"))))  # hey
