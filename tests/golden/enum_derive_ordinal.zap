# #1375: a derive using `comptime v.index` — each variant's 0-based declaration
# index (#1066 order). Exercises per-variant comptime data and deterministic,
# declaration-order variant iteration over a tier-1 enum.
@derivable(Ordinal)
def _derive_ordinal[T](T value) -> int64:
    match value:
        case comptime v in reflect.type_info(T).variants:
            return comptime v.index
    return -1

interface Ordinal:
    def ordinal(self) -> int64

@derive(Ordinal)
enum Color:
    Red
    Green
    Blue

def main():
    print(Color.Red.ordinal())
    print(Color.Green.ordinal())
    print(Color.Blue.ordinal())
