# #1394: recursion across kinds — an enum variant payload that is a derived
# struct recurses through the payload's own derived `sum()`, all from one
# generator that dispatches on kind.
@derivable(Sum)
def _derive_sum[T](T value) -> int64:
    if reflect.type_info(T).kind.name == "enum":
        match value:
            case comptime v in reflect.type_info(T).variants as payload:
                int64 acc = 0
                comptime for f in v.fields:
                    acc = acc + reflect.field(payload, comptime f.index).sum()
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
enum E:
    Nothing
    Wrap(P p)

def main():
    print(E.Wrap(P(3, 4)).sum())
    print(E.Nothing.sum())
