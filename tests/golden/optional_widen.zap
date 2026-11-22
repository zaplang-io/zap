# Regression for #1265: a bare value of type `T` widens to `Optional[T]`
# (implicit `Some` wrapping) in every type-contextual position — a `return`, an
# `Optional[T]` parameter, a typed binding, and a list-literal element — matching
# Python's `x: Optional[int] = 1` and completing the bare-`None` widening. These
# positions used to be clean sema type errors (ZAP4003 / ZAP4021 / ZAP4001);
# sema now admits them (payload-blind, exactly like an explicit `Optional.Some`),
# and the IR wrap (#1262, `gen_opt_some`) tags the payload. This golden reads
# every widened position natively (is_some / unwrap / unwrap_or) so the
# tagged-vs-raw storage is actually observed — scalar + reference payloads, and
# the adversarial zero.

struct Item:
    int64 value

def make_int() -> Optional[int64]:
    return 41

def make_zero() -> Optional[int64]:
    return 0

def make_str() -> Optional[str]:
    return "ret"

def make_item() -> Optional[Item]:
    return Item(7)

def take(Optional[int64] x) -> int64:
    return x.unwrap_or(-1)

def main():
    # Return a bare payload: scalar, the adversarial zero, a str, a struct.
    print(make_int().unwrap())  # 41
    print(make_zero().is_some())  # True
    print(make_zero().unwrap())  # 0
    print(make_str().unwrap())  # ret
    print(make_item().unwrap().value)  # 7

    # Bare payload to an Optional[T] parameter, with explicit Some and None.
    print(take(4))  # 4
    print(take(Optional.Some(8)))  # 8
    print(take(None))  # -1

    # Typed bindings: scalar and reference payload.
    Optional[int64] b = 5
    print(b.unwrap())  # 5
    Optional[str] s = "bound"
    print(s.unwrap())  # bound

    # List-literal elements: bare payloads mixed with None and an explicit Some.
    list[Optional[int64]] xs = [1, 0, None, Optional.Some(9)]
    print(xs[0].unwrap())  # 1
    print(xs[1].unwrap())  # 0
    print(xs[2].is_none())  # True
    print(xs[3].unwrap())  # 9
