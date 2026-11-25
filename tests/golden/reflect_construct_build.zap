# #1454 (E4 enabler): the reflect value-construction + field-write surface — the
# write-counterpart of `reflect.field` read (#1252). A `@derivable` generator
# zero-constructs a value of T with `reflect.construct(T)`, writes each field with
# `reflect.set_field(out, comptime f.index, …)` (which desugars to the ordinary
# field assignment `out.<name> = …`), then reads the fields back. This is the
# primitive a codec decoder builds on. (The generator returns a scalar rather than
# `Self` only to avoid an unrelated pre-existing gap in derived-method `Self`
# returns, #1455.)
@derivable(Build)
def _derive_build[T](T self, list[int64] src) -> int64:
    auto out = reflect.construct(T)
    comptime for f in reflect.type_info(T).fields:
        reflect.set_field(out, comptime f.index, src[comptime f.index])
    int64 total = 0
    comptime for f in reflect.type_info(T).fields:
        total = total + reflect.field(out, comptime f.index)
    return total

interface Build:
    def build(self, list[int64] src) -> int64

@derive(Build)
struct P:
    int64 x
    int64 y
    int64 z

def main():
    auto seed = P(0, 0, 0)
    print(seed.build([10, 20, 30]))
