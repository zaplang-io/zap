# Regression for #1195: a function whose declared return type is a tuple of
# `Optional[T]` must be able to `return (None, None)` — the declared tuple type
# supplies each element's `Optional[T]` context so a bare `None` element lowers
# to the canonical empty option (the tagged `None` for a scalar payload, the
# null pointer for a reference payload), not bail at IR lowering with ZAP6001
# ("this expression is not supported by codegen yet (Phase 1 slice)"). Covers
# pointer payloads (`bytes`, `str`, a reference struct) and scalar payloads
# (`int64`, `bool`), one- and two-missing-element tuples, a present+missing
# mix, the all-present `Optional.Some(...)` control (must stay unchanged), a
# typed tuple binding, and destructuring the result with
# `.is_none()`/`.is_some()`/`.unwrap()`. This unblocks the pymemcache-style
# `gets(key)` of zaplang-io/memcached, which returns a pair of missing values
# on a cache miss.

struct Item:
    int value


def miss_bytes() -> (Optional[bytes], Optional[bytes]):
    return (None, None)


def hit_bytes() -> (Optional[bytes], Optional[bytes]):
    return (Optional.Some(b"value"), Optional.Some(b"cas"))


def mixed_bytes() -> (Optional[bytes], Optional[bytes]):
    return (Optional.Some(b"value"), None)


def miss_str() -> (Optional[str], Optional[str]):
    return (None, None)


def miss_ref() -> (Optional[Item], Optional[Item]):
    return (None, None)


def mixed_ref() -> (Optional[Item], Optional[Item]):
    return (None, Optional.Some(Item(42)))


def miss_int() -> (Optional[int64], Optional[int64]):
    return (None, None)


def mixed_int() -> (Optional[int64], Optional[int64]):
    return (Optional.Some(7), None)


def miss_bool() -> (Optional[bool], Optional[bool]):
    return (None, None)


def main():
    # Pointer payload (bytes): both missing — the issue's minimal repro.
    auto value, cas = miss_bytes()
    print(value.is_none())  # True
    print(cas.is_none())  # True

    # Pointer payload (bytes): all present — the control that already worked.
    auto hv, hc = hit_bytes()
    print(hv.is_some())  # True
    print(hv.unwrap())  # value
    print(hc.unwrap())  # cas

    # Pointer payload (bytes): present + missing in one tuple.
    auto mv, mc = mixed_bytes()
    print(mv.is_some())  # True
    print(mv.unwrap())  # value
    print(mc.is_none())  # True

    # Pointer payload (str): both missing.
    auto sa, sb = miss_str()
    print(sa.is_none())  # True
    print(sb.is_none())  # True

    # Reference-struct payload: both missing (uses the niche null).
    auto ra, rb = miss_ref()
    print(ra.is_none())  # True
    print(rb.is_none())  # True

    # Reference-struct payload: missing + present.
    auto xa, xb = mixed_ref()
    print(xa.is_none())  # True
    print(xb.is_some())  # True
    print(xb.unwrap().value)  # 42

    # Scalar payload (int64): both missing (uses the tagged discriminant None).
    auto ia, ib = miss_int()
    print(ia.is_none())  # True
    print(ib.is_none())  # True

    # Scalar payload (int64): present + missing.
    auto ja, jb = mixed_int()
    print(ja.is_some())  # True
    print(ja.unwrap())  # 7
    print(jb.is_none())  # True

    # Scalar payload (bool): both missing.
    auto ba, bb = miss_bool()
    print(ba.is_none())  # True
    print(bb.is_none())  # True

    # A typed tuple binding (not a return) takes the same contextual None path.
    (Optional[bytes], Optional[bytes]) pair = (None, None)
    print(pair[0].is_none())  # True
    print(pair[1].is_none())  # True
