# #1402: a generic enum constructed with EXPLICIT type arguments and a variant
# path — `Box[int64].Full(9)`, the payload-free `Box[int64].Empty`, and the
# multi-field `Box[int64].Pair(3, 4)`. None of these variants mentions `T` in a
# payload, so inference cannot recover the instance; the explicit type argument
# is the only way to name it. This also exercises generic-enum `@derive`
# end-to-end: the derived `sum()` runs on each explicitly-constructed instance.
@derivable(Sum)
def _derive_sum[T](T value) -> int64:
    match value:
        case comptime v in reflect.type_info(T).variants as payload:
            int64 acc = 0
            comptime for f in v.fields:
                acc = acc + reflect.field(payload, comptime f.index)
            return acc
    return 0

interface Sum:
    def sum(self) -> int64

@derive(Sum)
enum Box[T]:
    Empty
    Full(int64 a)
    Pair(int64 a, int64 b)

def main():
    print(Box[int64].Full(9).sum())
    print(Box[int64].Empty.sum())
    print(Box[int64].Pair(3, 4).sum())
