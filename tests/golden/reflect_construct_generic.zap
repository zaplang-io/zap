# #1561: `reflect.construct(T)` where `T` resolves to a generic struct instance
# (`Pair[int64, str]`) zero-constructs that monomorphized instance — the generic
# counterpart of the non-generic `reflect.construct` (#1454/#1490). Each field is
# zero-built with the instance's type arguments substituted in (`A := int64`,
# `B := str`), so a decoder can build a generic-struct base before populating it.
#
# Gated by the CI LLVM-18 matrix (Docker/LLVM down; `zap ir` proves lowering).

@derivable(Mk)
def _derive_mk[T](str s) -> T:
    return reflect.construct(T)

interface Mk:
    def mk(str s) -> Self

struct Pair[A, B]:
    A a
    B b

def main():
    # `Pair[int64, str]`: a -> 0 (int zero), b -> "" (str zero).
    auto p = _derive_mk[Pair[int64, str]]("")
    print(p.a == 0)
    print(p.b == "")
    # A second, distinct instantiation zero-builds independently.
    auto q = _derive_mk[Pair[str, int64]]("")
    print(q.a == "")
    print(q.b == 0)
