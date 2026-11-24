# Regression for #1407: a whole `Optional[U]` value is assignable to an
# `Optional[T]` slot ONLY when the payloads match (`U == T`). A mismatch —
# `Optional.Some(<bigint value>)` / `Optional.Some(<int32>)` / a `str` value into
# `Optional[int64]` — now fails to lower with `ZAP4001` instead of storing the
# `Optional$U` pointer raw into the `Optional$T` slot and miscompiling (the
# sibling of #1379). Those rejections are host lowering tests
# (crates/ir/tests/optional_payload_variance.rs); this golden pins that the
# ACCEPTED matching-payload forms still compile and run end-to-end, so the new
# `check_optional_payload` reject does not over-reject.


def main():
    # A genuine `Optional[bigint]` with a `bigint` value (the partner of the
    # rejected `Optional[int64] = Optional.Some(<bigint>)`).
    bigint g = 9223372036854775808
    Optional[bigint] a = Optional.Some(g)
    print(a.is_some())  # True
    print(a.unwrap())  # 9223372036854775808
    # A whole `Optional[int64]` copied into another `Optional[int64]` binding:
    # identical payload, never rejected.
    Optional[int64] b = Optional.Some(5)
    Optional[int64] c = b
    print(c.unwrap())  # 5
    # The #1265 bare-payload widening is untouched: a bare `int32` widens into
    # `Optional[int64]` (the value is not itself an `Optional`).
    int32 n = 7
    Optional[int64] d = n
    print(d.unwrap())  # 7
    # A nested `Optional[Optional[int64]]`: the top-level payloads match, so the
    # payload-variance check must not false-positive on the inner level.
    Optional[int64] inner = Optional.Some(42)
    Optional[Optional[int64]] nested = Optional.Some(inner)
    print(nested.is_some())  # True
    print(nested.unwrap().unwrap())  # 42
