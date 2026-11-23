# Golden fixture: a user-extensible `@derive(Dump)` on a GENERIC type (#1292,
# E3c). `@derive(Dump)` on `Pair[A, B]` synthesizes `dump(self)` forwarding to
# `_derive_dump[Pair[A, B]](self)` — the generator specialized at the type
# applied to its own parameters. Monomorphization specializes it at the concrete
# `Pair[int64, int64]`, so the generator's `comptime for` unrolls over the two
# fields structurally (no literal `[A: Dump]` bound is emitted; conformance is
# per-instantiation). So `p.dump()` prints each field.

@derivable(Dump)
def _derive_dump[T](T value):
    comptime for f in reflect.type_info(T).fields:
        print(reflect.field(value, comptime f.index))

interface Dump:
    def dump(self)

@derive(Dump)
struct Pair[A, B]:
    A first
    B second

def main():
    auto p = Pair(10, 20)
    p.dump()
