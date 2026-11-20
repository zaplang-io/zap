# Golden: associated / output types on interfaces (#1207).
#
# An interface declares an associated/output type with `type Out` and names it
# in a method signature. A conformer binds it STRUCTURALLY — `Out` is whatever
# the conformer's producing method returns — and bounded generic code names it
# as the type projection `T.Out`, without a second type parameter. This is the
# cleaner surface behind the reflected operator capabilities (#1199's
# `RMul[L, Out]` could become `RMul[L]` + `type Out`).
#
# At monomorphization the projection `T.Out` in `build`'s signature resolves to
# the concrete binding (Factory.make returns Widget), so codegen sees an ordinary
# concrete return type — associated types add no runtime mechanism. GC is
# unaffected (the heap Widget is traced by its per-struct descriptor).

interface Producer[In]:
    type Out
    def make(self, In seed) -> Out

struct Widget:
    int64 n
    def show(self) -> int64:
        return self.n

struct Factory:
    int64 base
    def make(self, f64 seed) -> Widget:
        return Widget(n=self.base)

# `T.Out` names the produced type; `T` is constrained to produce from an f64.
def build[T: Producer[f64]](T f, f64 s) -> T.Out:
    return f.make(s)

def main():
    Widget w = build(Factory(base=42), 1.0)
    print(w.show())
