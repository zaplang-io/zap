# E4 (#1429/#1430): shadow-prove that the built-in `@derive(Repr)` can be
# expressed as a library `@derivable` generator, WITHOUT replacing the built-in.
# Now that `repr()` is a builtin (#1384), a generator can render each field in
# repr mode exactly as the built-in does, and name the type via
# `reflect.type_info(T).name`. A struct derives BOTH the built-in `Repr` (reached
# via the `repr()` builtin, which resolves its `__repr__`) and a library
# `StructRepr`; the two produce the SAME string. Built-in, the value-struct
# auto-derive and the prelude are untouched. Covers int / str / bool fields and a
# nested derived struct.
@derivable(StructRepr)
def _derive_structrepr[T](T value) -> str:
    str out = reflect.type_info(T).name + "("
    comptime for f in reflect.type_info(T).fields:
        if comptime f.index > 0:
            out = out + ", "
        out = out + f.name + "=" + repr(reflect.field(value, comptime f.index))
    return out + ")"

interface StructRepr:
    def structrepr(self) -> str

@derive(Repr)
struct Inner:
    int64 a

@derive(Repr, StructRepr)
struct P:
    int64 x
    str s
    bool b
    Inner inner

def main():
    auto p = P(1, "a", True, Inner(5))
    auto q = P(2, "z", False, Inner(9))
    # The library shadow matches the built-in `repr()` for each struct...
    print(repr(p) == p.structrepr())
    print(repr(q) == q.structrepr())
    # ...and the rendering is non-vacuous: different structs render differently.
    print(repr(p) == q.structrepr())
