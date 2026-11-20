# Golden: generic-interface bounds with concrete type arguments (#1205).
#
# A bound may name a GENERIC interface instantiated at a concrete type,
# `[T: Container[int64]]`. The interface's own type parameter is substituted
# (E := int64) when the bound is checked at each instantiation, so a type whose
# `get` returns int64 conforms to `Container[int64]` and a type whose `get`
# returns str conforms instead to `Container[str]`. Conformance is enforced by
# the monomorphizer per concrete instance (static dispatch, no fat pointer); the
# bound adds no runtime mechanism (GC unaffected under both collectors). The
# bounded value flows THROUGH the generic unchanged and its method is called on
# the concrete type at the use site — resolving a call through the type
# parameter itself is a separate slice (#1206).

interface Container[E]:
    def get(self) -> E

struct IntBox:
    int64 v
    def get(self) -> int64:
        return self.v

struct StrBox:
    str s
    def get(self) -> str:
        return self.s

# Bounded generic function: T must provide Container's methods with E := int64,
# i.e. a `get(self) -> int64`. IntBox satisfies it; StrBox would not.
def keep_int[T: Container[int64]](T c) -> T:
    return c

# The same interface at a different concrete argument: here E := str, so StrBox
# conforms and IntBox would not. Two instantiations prove per-argument
# substitution and that the monomorphizer emits a distinct instance each time.
def keep_str[T: Container[str]](T c) -> T:
    return c

def main():
    IntBox ib = keep_int(IntBox(v=7))
    StrBox sb = keep_str(StrBox(s="boxed"))
    print(ib.get())
    print(sb.get())
