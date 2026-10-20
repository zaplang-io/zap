# Golden: interface bounds on type parameters (generics slice 5, I#164).
#
# A bound `[T: Display]` is enforced at each concrete instantiation by the same
# structural-conformance rule as interface values (I#84/D-0016), but the bound's
# methods resolve STATICALLY per instance — a direct call to the concrete
# method, no fat pointer / vtable (design/0012 slice 5). Two implementors
# (Greeter, Stamp) instantiate both a bounded FUNCTION (`announce[T]`) and a
# bounded TYPE (`Box[T]`) at different concrete types, so the monomorphizer
# emits a separate, directly-dispatched copy per instance. The heap `str` fields
# are traced by the existing per-struct GC descriptors — bounds add no new GC
# mechanism (GC unaffected under both collectors).

interface Display:
    def show(self) -> str

struct Greeter:
    str msg
    def show(self) -> str:
        return self.msg

struct Stamp:
    str label
    def show(self) -> str:
        return self.label

# Bounded generic FUNCTION: the body calls the bound method on a value of T.
def announce[T: Display](T x) -> str:
    return x.show()

# Bounded generic TYPE: the method calls the bound method on a T-typed field.
struct Box[T: Display]:
    T item
    def reveal(self) -> str:
        return self.item.show()

def main():
    Greeter g = Greeter(msg="hello")
    Stamp s = Stamp(label="stamped")
    print(announce(g))
    print(announce(s))
    Box[Greeter] bg = Box(Greeter(msg="boxed-greeter"))
    Box[Stamp] bs = Box(Stamp(label="boxed-stamp"))
    print(bg.reveal())
    print(bs.reveal())
