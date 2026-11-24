# #1394: one `@derivable` generator handles both structs and enums by dispatching
# on `reflect.type_info(T).kind` at its top level (the dead-kind branch is pruned
# per instantiation). `Sum` folds int64 fields of a struct, or the payload of the
# active enum variant.
@derivable(Sum)
def _derive_sum[T](T value) -> int64:
    if reflect.type_info(T).kind.name == "enum":
        match value:
            case comptime v in reflect.type_info(T).variants as payload:
                int64 acc = 0
                comptime for f in v.fields:
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
struct Pair:
    int64 a
    int64 b

@derive(Sum)
enum N:
    Zero
    One(int64 a)
    Two(int64 a, int64 b)

def main():
    print(Pair(10, 20).sum())
    print(N.Zero.sum())
    print(N.One(5).sum())
    print(N.Two(3, 4).sum())
