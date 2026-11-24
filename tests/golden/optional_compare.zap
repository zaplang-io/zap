# Regression for #1368: `==`/`!=` and ordering (`<` `<=` `>` `>=`) on
# `Optional[T]`. `==`/`!=` are structural (None == None; Some(x) == Some(y) iff
# x == y; Some never equals None); ordering is a total order with None before
# every Some (Option<T>-style: None < Some(_), and Some(x) OP Some(y) is
# x OP y). A bare payload on the right (`opt == 7`) compares as `opt == Some(7)`.
# Previously these bailed at lowering with ZAP6001 ("Optional$T does not define
# '__eq__'/'__cmp__'"); only `== None` / `is None` were special-cased.


def main():
    Optional[int64] a = Optional.Some(1)
    Optional[int64] a2 = Optional.Some(1)
    Optional[int64] b = Optional.Some(2)
    Optional[int64] n = None
    Optional[int64] n2 = None

    # Equality (structural).
    print(a == a2)  # True
    print(a == b)  # False
    print(a != b)  # True
    print(a == n)  # False
    print(n == n2)  # True
    print(n != a)  # True

    # Bare payload on the right: `opt == Some(payload)`.
    print(a == 1)  # True
    print(a == 2)  # False
    print(n == 1)  # False

    # Ordering: None sorts before every Some.
    print(a < b)  # True
    print(b < a)  # False
    print(a <= a2)  # True
    print(a >= a2)  # True
    print(n < a)  # True
    print(a < n)  # False
    print(n <= n2)  # True
    print(n < n2)  # False
    print(a > n)  # True

    # Reference (str) payload: equality and ordering via the payload's operator.
    Optional[str] s1 = Optional.Some("apple")
    Optional[str] s2 = Optional.Some("banana")
    print(s1 == s1)  # True
    print(s1 < s2)  # True
    print(s1 == "apple")  # True
