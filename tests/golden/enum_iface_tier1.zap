# Golden fixture: a tier-1 (unboxed-integer) enum conforms to an interface and is
# usable as an interface value (#731, D-0016 extended to enums). A tier-1 enum is
# a bare backing integer with no pointer identity, so to put it behind an
# interface's `{data, itable}` fat pointer its scalar tag is boxed into a small
# heap cell `{Enum}$box = { $tag:i64 }`; dispatch then goes through the itable's
# per-method thunk `{Enum}$box__m`, which reloads the tag and tail-calls the real
# `{Enum}__m(self)`. The `match self` body proves the real method runs on the
# concrete member behind the interface.

interface Ranked:
    def rank(self) -> int

enum Color:
    Red
    Green
    Blue

    def rank(self) -> int:
        match self:
            case Color.Red:
                return 1
            case Color.Green:
                return 2
            case _:
                return 3

# Round-trip: pass the enum as an interface-typed parameter and dispatch.
def rank_of(Ranked r) -> int:
    return r.rank()

def main():
    # A direct method call on a bare tier-1 enum value (this itself was ZAP6001
    # before #731 — tier-1 enum methods did not lower at all).
    print(Color.Green.rank())  # 2

    # Dispatch through an interface-typed binding.
    Ranked a = Color.Red
    Ranked b = Color.Blue
    print(a.rank())  # 1
    print(b.rank())  # 3

    # Re-bind the SAME interface variable: dispatch follows the boxed value.
    a = Color.Green
    print(a.rank())  # 2

    # Round-trip through an interface-typed parameter, each widening boxed fresh.
    print(rank_of(Color.Red))  # 1
    print(rank_of(Color.Green))  # 2
    print(rank_of(Color.Blue))  # 3
