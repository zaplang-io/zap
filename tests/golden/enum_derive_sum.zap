# #1374: an enum `@derive` generator discriminates variants at runtime via
# `match value: case comptime v in reflect.type_info(T).variants as payload:`,
# reading each variant's payload with `reflect.field(payload, i)`. Covers a
# tier-1 (unit) variant, a one-field payload, and a two-field payload.
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
enum N:
    Zero
    One(int64 a)
    Two(int64 a, int64 b)

def main():
    print(N.Zero.sum())
    print(N.One(5).sum())
    print(N.Two(3, 4).sum())
