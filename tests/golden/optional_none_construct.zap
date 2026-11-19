# Regression for #1075: a bare `None` supplied for an `Optional[T]`
# struct-constructor field must lower to the canonical empty option (the tagged
# `None` for a scalar payload, the null pointer for a reference payload), not
# bail at IR lowering with ZAP6001. Covers the positional `Box(None, ...)` and
# keyword `Box(scalar=None, ...)` forms, a scalar payload (`Optional[int]`) and
# a reference payload (`Optional[Item]`), the omitted-field default-`None` path,
# and Some controls (scalar + reference) mixed with None in one constructor.

struct Item:
    int value

struct Box:
    Optional[int] scalar
    Optional[Item] slot

struct Defaulted:
    Optional[int] scalar = None
    Optional[Item] slot = None

def main():
    # Positional: both fields a bare None.
    Box a = Box(None, None)
    print(a.scalar.is_none())  # True
    print(a.slot.is_none())  # True

    # Keyword: both fields a bare None.
    Box b = Box(scalar=None, slot=None)
    print(b.scalar.is_none())  # True
    print(b.slot.is_none())  # True

    # Some scalar control alongside a None reference.
    Box c = Box(Optional.Some(7), None)
    print(c.scalar.is_some())  # True
    print(c.scalar.unwrap_or(-1))  # 7
    print(c.slot.is_none())  # True

    # Keyword Some reference control alongside a None scalar.
    Box d = Box(scalar=None, slot=Optional.Some(Item(42)))
    print(d.scalar.is_none())  # True
    print(d.slot.is_some())  # True
    print(d.slot.unwrap().value)  # 42

    # Omitted fields fall back to their declared `None` defaults.
    Defaulted e = Defaulted()
    print(e.scalar.is_none())  # True
    print(e.slot.is_none())  # True
