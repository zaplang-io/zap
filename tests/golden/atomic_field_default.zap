# Golden (#1152): a struct field whose default is a generic constructor is
# evaluated during construction, both when the caller omits the field and when
# it supplies one explicitly. Before #1152 the field-default initializer was
# left un-monomorphized, so the generic `Atomic(...)` constructor reached
# codegen as a bare "call to unknown function 'Atomic'".
#
# Covers: omitted vs explicit initializers; independent per-instance atomic
# state across two instances (the point for memcached's per-client socket
# state, zaplang-io/memcached#2); multiple Atomic fields; two Atomic widths
# (int64 and bool); Atomic fields alongside non-atomic defaulted fields; a
# non-zero `Atomic(N)` default; and a user generic-struct default (`Box(5)`) —
# the fix routes *every* generic constructor in a field default through the
# same monomorphization as a `let` initializer, not just `Atomic`.


def b2i(bool x) -> int64:
    if x:
        return 1
    return 0


struct Box[T]:
    T value


struct Client:
    Atomic[int64] state = Atomic(0)
    Atomic[int64] generation = Atomic(7)
    Atomic[bool] closed = Atomic(False)
    int64 id = 42
    str name = "client"
    Box[int64] boxed = Box(5)


def main():
    # Fully omitted: every field comes from its default initializer.
    auto a = Client()
    print(a.state.load())  # 0
    print(a.generation.load())  # 7
    print(b2i(a.closed.load()))  # 0
    print(a.id)  # 42
    print(a.name)  # client
    print(a.boxed.value)  # 5

    # Independence: mutate a's atomic state, then build a second instance. The
    # second instance must see pristine defaults — the defaults are evaluated
    # fresh per construction, so the two clients share nothing.
    a.state.store(99)
    print(a.generation.add(3))  # 10 (returns the new value)
    a.closed.store(True)
    auto b = Client()
    print(a.state.load())  # 99
    print(a.generation.load())  # 10
    print(b2i(a.closed.load()))  # 1
    print(b.state.load())  # 0  — independent of a
    print(b.generation.load())  # 7
    print(b2i(b.closed.load()))  # 0

    # Explicit initializer: supplying the field at construction behaves exactly
    # like the omitted/defaulted form.
    auto c = Client(state=Atomic(0), generation=Atomic(7))
    print(c.state.load())  # 0
    print(c.generation.load())  # 7
    print(c.boxed.value)  # 5  — the omitted field still defaults
