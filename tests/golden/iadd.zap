# Golden fixture: in-place augmented assignment (ticket #61, D-0047/D-0023).
#
# `a OP= b` calls the in-place dunder `a.__iOP__(b)` when the LHS type defines
# it (mutating the operand in place, no fresh allocation), and otherwise falls
# back to deriving `a = a.__OP__(b)` from the binary operator.
#
# The two behaviours are observable through an alias: a plain `struct` is a
# reference type (D-0023), so `auto alias = a` shares identity. An in-place
# `+=`/`-=` mutates the shared instance, so the change is visible through the
# alias; the derive-from-binary fallback rebinds the local to a *new* instance,
# leaving the alias untouched.

# Defines the in-place dunders: `+=` and `-=` mutate `self` and return it.
struct Acc:
    int64 total

    def __iadd__(self, Acc other) -> Acc:
        self.total = self.total + other.total
        return self

    def __isub__(self, Acc other) -> Acc:
        self.total = self.total - other.total
        return self

# Defines only the binary `__add__`: `+=` falls back to `v = v.__add__(...)`.
struct Vec:
    int64 x

    def __add__(self, Vec other) -> Vec:
        return Vec(x=self.x + other.x)

def main():
    auto a = Acc(total=10)
    auto alias = a            # shares identity with `a` (reference struct)
    auto b = Acc(total=5)

    a += b                    # in-place: mutates the shared instance
    print(a.total)            # 15
    print(alias.total)        # 15 — the in-place effect is visible via the alias

    a -= Acc(total=3)         # in-place again
    print(a.total)            # 12
    print(alias.total)        # 12

    auto v = Vec(x=1)
    auto valias = v           # shares identity with `v`
    v += Vec(x=2)             # fallback: rebinds `v` to a fresh Vec, no mutation
    print(v.x)                # 3
    print(valias.x)           # 1 — the original instance is left unchanged
