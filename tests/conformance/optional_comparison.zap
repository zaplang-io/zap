# claim: OPT-2
# SPEC §4.6 (#1368): `Optional[T]` compares with `==`/`!=` structurally
# (None == None; Some(x) == Some(y) iff x == y; Some never equals None) and with
# the ordering operators as a total order with `None` before every `Some`
# (None < Some(_); Some(x) OP Some(y) is x OP y) — matching Rust's Option<T> and
# DELIBERATELY diverging from Python, which raises TypeError ordering None
# against a value. A bare payload on the right compares as if wrapped
# (`opt == 7` is `opt == Some(7)`). This pins the intentional divergence.
def main():
    Optional[int64] s1 = Optional.Some(1)
    Optional[int64] s1b = Optional.Some(1)
    Optional[int64] s2 = Optional.Some(2)
    Optional[int64] n = None

    # Structural equality.
    print(s1 == s1b)  # True
    print(s1 == s2)  # False
    print(s1 != s2)  # True
    print(s1 == n)  # False
    print(n == None)  # True

    # Bare payload on the right.
    print(s1 == 1)  # True
    print(s1 == 2)  # False

    # Total order: None before every Some (the deliberate Python divergence —
    # Python would raise ordering None against an int).
    print(n < s1)  # True
    print(s1 < n)  # False
    print(n <= n)  # True
    print(s1 < s2)  # True
    print(s2 > s1)  # True
    print(s1 >= s1b)  # True
