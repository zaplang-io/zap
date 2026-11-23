# Golden fixture: a `@derive` generator that guards on a field's comptime kind
# and would abort with `comptime.error` on an unsupported (nested-struct) field
# (#1330). `Flat` has only primitive fields, so the `f.ty.kind.name == "struct"`
# guard is comptime-false for every field and prunes away (the guarded
# `comptime.error` is dead), leaving the generator to print each field. This
# exercises the wired comptime surface end-to-end: `comptime for` + a
# comptime-constant `if` resolved at specialization time + registry-backed
# `f.ty.kind` + `reflect.field`.

@derivable(Ser)
def _derive_ser[T](T value):
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == "struct":
            comptime.error(f.span, "Ser: nested struct field unsupported")
        print(reflect.field(value, comptime f.index))

interface Ser:
    def ser(self)

@derive(Ser)
struct Flat:
    int64 a
    int64 b

def main():
    auto x = Flat(10, 20)
    x.ser()
