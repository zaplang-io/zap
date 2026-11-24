# #1401: enum->enum recursion in a `@derive` generator. A variant payload that
# is itself a derived enum recurses through that enum's own derived `sum()` — the
# generated `$cvp` payload capture is enum-typed and dispatches its method
# correctly (NOT a core defect: the original repro only failed because its
# generator called `.sum()` on a scalar leaf field, which has no such method).
# A per-field comptime kind check routes scalar leaves to a direct add and
# struct/enum payloads to a recursive `.sum()`, so one generator spans a struct,
# an enum with a scalar-leaf variant, and an enum nesting another enum.
@derivable(Sum)
def _derive_sum[T](T value) -> int64:
    if reflect.type_info(T).kind.name == "enum":
        match value:
            case comptime v in reflect.type_info(T).variants as payload:
                int64 acc = 0
                comptime for f in v.fields:
                    if f.ty.kind.name == "struct" or f.ty.kind.name == "enum":
                        acc = acc + reflect.field(payload, comptime f.index).sum()
                    else:
                        acc = acc + reflect.field(payload, comptime f.index)
                return acc
        return 0
    else:
        int64 sacc = 0
        comptime for f in reflect.type_info(T).fields:
            sacc = sacc + reflect.field(value, comptime f.index)
        return sacc

interface Sum:
    def sum(self) -> int64

@derive(Sum)
struct P:
    int64 x
    int64 y

@derive(Sum)
enum Inner:
    Leaf(int64 a)
    Pair(P p)

@derive(Sum)
enum Outer:
    O(Inner inner)

def main():
    print(Outer.O(Inner.Leaf(42)).sum())
    print(Outer.O(Inner.Pair(P(3, 4))).sum())
    print(Inner.Leaf(9).sum())
    print(Inner.Pair(P(10, 20)).sum())
