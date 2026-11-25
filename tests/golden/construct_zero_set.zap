# E4 (#1429/#1490): runtime proof that `reflect.construct(T)` zero-constructs the
# widened field-kind set. #1454's v1 zeroed only scalar / str / bytes fields and
# errored cleanly for anything else; #1490 widens the zero set to a nested struct,
# an `Optional[T]`, a `list` / `dict` / `set`, and a tuple.
#
# A `@derivable` generator with a `Self` return (#1455) zero-constructs a `Wide`
# and `main` reads each field back with a concrete accessor — no DOM / dict lookup,
# so the readback is independent of the codec shadows. The expected output pins the
# zeros: an integer `0`, a zero-valued nested struct (`0` and the empty string),
# empty collections (`len == 0` for the list, dict and set), and an empty optional
# (`None`). This is the end-to-end gate for Layer 1 of #1490.

@derivable(Make)
def _derive_make[T]() -> T:
    return reflect.construct(T)

interface Make:
    def make() -> Self

@derive(Eq, Hash)
struct Inner:
    int64 a
    str s

@derive(Make)
struct Wide:
    int64 n
    Inner inner
    Optional[int64] opt
    list[int64] xs
    dict[str, int64] m
    set[int64] st
    (int64, str) pair

def main():
    auto z = Wide.make()
    # scalar zero
    print(z.n)
    # nested struct zero-constructed (recursion): its own fields are zeroed
    # (the str field's length, to keep every line an unambiguous token)
    print(z.inner.a)
    print(len(z.inner.s))
    # collections start empty
    print(len(z.xs))
    print(len(z.m))
    print(len(z.st))
    # tuple element zeros (int element, then the str element's length)
    print(z.pair[0])
    print(len(z.pair[1]))
    # optional starts as the empty optional
    match z.opt:
        case Optional.None:
            print("none")
        case Optional.Some(v):
            print(v)
