# E4 (#1429/#1431): shadow-prove that the built-in `@derive(Eq)` can be expressed
# as a plain library `@derivable` generator, WITHOUT replacing the built-in. A
# struct derives BOTH the compiler built-in `Eq` (used through `==`) and a library
# `StructEq` whose generator compares fields structurally; the two must agree for
# every case. This dogfoods the mechanism over the compiler's own structural
# equality while leaving the built-in (and the implicit value-struct auto-derive)
# untouched. Covers int / str / bool / float fields and a nested derived struct.
@derivable(StructEq)
def _derive_structeq[T](T a, T b) -> bool:
    comptime for f in reflect.type_info(T).fields:
        if reflect.field(a, comptime f.index) != reflect.field(b, comptime f.index):
            return False
    return True

interface StructEq:
    def structeq(self, Self other) -> bool

@derive(Hash, Eq)
struct Inner:
    int64 a

@derive(Eq, StructEq)
struct P:
    int64 x
    str s
    bool b
    float f
    Inner inner

def main():
    auto p = P(1, "a", True, 2.5, Inner(5))
    auto q = P(1, "a", True, 2.5, Inner(5))
    auto r = P(1, "a", True, 2.5, Inner(6))
    auto t = P(1, "z", True, 2.5, Inner(5))
    # Each pair prints the built-in `==` then the library shadow `.structeq()`;
    # the two lines must match.
    print(p == q)
    print(p.structeq(q))
    print(p == r)
    print(p.structeq(r))
    print(p == t)
    print(p.structeq(t))
