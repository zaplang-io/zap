# Golden fixture (#1489 Epic, #1493 part A): dereferencing a null built-in
# reference VALUE — a non-`Optional` `str`/`list`/`dict`/… field left null, then
# `len()`'d, subscripted, or iterated — raises a catchable, located
# `NullReferenceError` instead of a SIGSEGV. The owner here (`n`/`x`) is non-null;
# only the inner reference value is null, so the guard fires at the value-deref
# site, not the field read. Catch behaviour is deterministic; the located message
# (with the source line) is pinned by `null_deref_len_uncaught`.
struct Name:
    str value = None

struct Ints:
    list[int64] items = None

def main() -> int64:
    # len() of a null str (the Epic's headline `len(n.name)`) is caught.
    Name n = Name()
    try:
        auto k = len(n.value)
        print(k)
    except NullReferenceError:
        print("caught len")

    # Subscripting a null list is caught.
    Ints x = Ints()
    try:
        auto v = x.items[0]
        print(v)
    except NullReferenceError:
        print("caught index")

    # Iterating a null list is caught (as the Exception base).
    try:
        for y in x.items:
            print(y)
    except Exception:
        print("caught iter")

    # Non-null references work normally.
    Name m = Name(value="hi")
    print(len(m.value))
    Ints z = Ints(items=[10, 20])
    print(z.items[0])
    return 0
