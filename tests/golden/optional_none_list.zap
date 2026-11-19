# Regression for #1080: a bare `None` as a `list[Optional[T]]` element must
# lower to the canonical empty option, in both the list-literal form `[None]`
# and the comprehension form `[None for _ in range(n)]`. Covers a reference
# payload, mixing None and Some (position-preserving reads), a bare `None`
# passed to `.append()`, and reads + writes across a growth well past the
# initial capacity.

struct Item:
    int value

def main():
    # List literal, a single bare None.
    list[Optional[Item]] a = [None]
    print(len(a))  # 1
    print(a[0].is_none())  # True

    # Comprehension form.
    list[Optional[Item]] b = [None for index in range(3)]
    print(len(b))  # 3
    print(b[0].is_none())  # True
    print(b[2].is_none())  # True

    # Mixing None and Some in one literal preserves positions.
    list[Optional[Item]] c = [
        None,
        Optional.Some(Item(1)),
        None,
        Optional.Some(Item(2)),
    ]
    print(c[0].is_none())  # True
    print(c[1].is_some())  # True
    print(c[1].unwrap().value)  # 1
    print(c[2].is_none())  # True
    print(c[3].unwrap().value)  # 2

    # Writes across a growth past the initial capacity: append a bare `None`
    # and `Optional.Some(...)` alternately, then read every slot back.
    list[Optional[Item]] d = [None]
    int index = 0
    while index < 50:
        if index % 2 == 0:
            d.append(None)
        else:
            d.append(Optional.Some(Item(index)))
        index = index + 1
    print(len(d))  # 51
    print(d[0].is_none())  # True
    print(d[1].is_none())  # True
    print(d[2].is_some())  # True
    print(d[2].unwrap().value)  # 1
    print(d[50].is_some())  # True
    print(d[50].unwrap().value)  # 49
