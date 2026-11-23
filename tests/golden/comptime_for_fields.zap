# Golden fixture: the `comptime for` surface over `reflect.type_info(T).fields`
# (E2/E3 compile-time reflection, #1298 / epic #1296). The loop is
# compile-time-unrolled over `Point`'s two declared fields, and each
# `reflect.field(p, comptime f.index)` desugars to a typed, unboxed field access
# (`p.x` / `p.y`). So this prints the two field values in declaration order —
# the same on every arch/GC. End-to-end proof that the parsed comptime surface
# runs through sema, mono unrolling, lowering and codegen.
struct Point:
    int64 x
    int64 y

def main():
    auto p = Point(10, 20)
    comptime for f in reflect.type_info(Point).fields:
        print(reflect.field(p, comptime f.index))
