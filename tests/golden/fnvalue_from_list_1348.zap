# Calling a first-class function value read back from a typed collection (#1348).
#
# A `fn(..) -> R` value stored in a `list`/`dict` (or a collection-typed struct
# field) and read back by iteration or indexing used to lose its function type
# (the element kind collapsed to a bare pointer), so the call site fell back to
# a top-level name lookup and failed with ZAP6001. A dedicated `Elem::Func`
# element kind now preserves the signature, so the call lowers to an indirect
# closure call and actually runs. This fixture proves every read form executes:
# a local list (iterated and indexed), a struct-field list, an `auto` binding,
# a callable with parameters, a non-scalar (`list`) result, a dict value, and
# the otel #3 observable-gauge shape (a list of callbacks each returning a list).


def sample() -> int64:
    return 42


def dbl(int64 x) -> int64:
    return x * 2


def observe() -> list[int64]:
    return [1, 2, 3]


struct Meter:
    list[fn() -> list[int64]] gauges


def main():
    # Local list, iterated.
    list[fn() -> int64] callbacks = [sample]
    for callback in callbacks:
        print(callback())
    # Indexed directly.
    print(callbacks[0]())
    # Indexed into an unannotated `auto` binding, then called.
    auto first = callbacks[0]
    print(first())
    # A callable with a parameter, iterated and indexed.
    list[fn(int64) -> int64] fns = [dbl]
    for f in fns:
        print(f(21))
    print(fns[0](21))
    # A dict value that is a function value.
    dict[str, fn() -> int64] handlers = {"a": sample}
    print(handlers["a"]())
    # The otel #3 shape: a struct-field list of callbacks each returning a list.
    Meter m = Meter([observe])
    for g in m.gauges:
        print(g())
    print(m.gauges[0]())
