# Golden fixture: a GENERIC `comptime for` generator unrolls per instantiation
# (#1291, E3b generic-T resolution). `dump[T]` iterates `reflect.type_info(T)`'s
# fields; called at `Point`, `T` resolves to `Point` through monomorphization's
# active type-parameter substitution, so the loop unrolls over `Point`'s two
# fields and each `reflect.field(p, comptime f.index)` becomes an unboxed `p.x`
# / `p.y`. This is the generator shape a real `@derive` forwarding stub targets.
struct Point:
    int64 x
    int64 y

def dump[T](T p):
    comptime for f in reflect.type_info(T).fields:
        print(reflect.field(p, comptime f.index))

def main():
    dump(Point(10, 20))
