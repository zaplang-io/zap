# Golden fixture: a user-extensible `@derive(Dump)` end-to-end (#1291, E3b).
# `_derive_dump[T]` is a library generator registered with `@derivable(Dump)`,
# and `interface Dump` declares the single method the trait requires. `@derive`
# on `Point` synthesizes a thin `dump(self)` forwarding stub that calls
# `_derive_dump[Point](self)`; that generator's `comptime for` unrolls over
# `Point`'s fields (the generic-`T` resolution, #1291 part 1), so `p.dump()`
# prints each field in turn.
#
# The generator parameter is named `value`, not `self`: a generic free function
# whose parameter is named `self` currently mis-lowers `reflect.field` (ZAP6001
# "field access on a non-struct value") — a pre-existing reflect-surface defect
# orthogonal to the forwarding-stub synthesis, reported separately.

@derivable(Dump)
def _derive_dump[T](T value):
    comptime for f in reflect.type_info(T).fields:
        print(reflect.field(value, comptime f.index))

interface Dump:
    def dump(self)

@derive(Dump)
struct Point:
    int64 x
    int64 y

def main():
    auto p = Point(10, 20)
    p.dump()
