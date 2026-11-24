# E4 (#1429/#1443): shadow-prove that the built-in `@derive(Ord)` can be expressed
# as a library `@derivable` generator, WITHOUT replacing the built-in. A struct
# derives BOTH the built-in `Ord` (used through `<` / `==`) and a library
# `StructOrd` whose generator compares fields three-way; the two must agree on
# ORDERING (the sign of the comparison — `zap_str_cmp` is not normalized to +-1,
# so raw `__cmp__` values are not compared). Built-in, auto-derive and prelude are
# untouched. Covers an int-primary ordering and a str tiebreak, both directions.
@derivable(StructOrd)
def _derive_structord[T](T a, T b) -> int64:
    comptime for f in reflect.type_info(T).fields:
        if reflect.field(a, comptime f.index) < reflect.field(b, comptime f.index):
            return -1
        if reflect.field(a, comptime f.index) > reflect.field(b, comptime f.index):
            return 1
    return 0

interface StructOrd:
    def structcmp(self, Self other) -> int64

@derive(Ord, Eq, StructOrd)
struct P:
    int64 x
    str s

def main():
    auto a = P(1, "m")
    auto b = P(2, "m")
    auto c = P(1, "a")
    auto d = P(1, "b")
    # Each built-in line is followed by the matching shadow line.
    print(a < b)
    print(a.structcmp(b) < 0)
    print(c < d)
    print(c.structcmp(d) < 0)
    print(a == a)
    print(a.structcmp(a) == 0)
    print(b < a)
    print(b.structcmp(a) < 0)
