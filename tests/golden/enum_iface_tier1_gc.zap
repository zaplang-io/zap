# Golden fixture: a boxed tier-1 enum behind an interface survives GC and keeps
# dispatching correctly (#731, D-0016; GC per D-0009/D-0140). Each widening of a
# tier-1 enum to an interface value allocates a fresh heap box `{Enum}$box`, so
# this loop produces a large volume of short-lived boxes — forcing collection
# cycles while live boxes (the current call's receiver) must stay traced and
# dispatch through the itable thunk. The box is a pointer-free leaf, so GC traces
# it trivially and it is rooted transitively through the fat pointer. Only the
# deterministic dispatch total is asserted (no heap-byte counts — those are
# non-deterministic; see GC golden policy).

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

def rank_of(Ranked r) -> int:
    return r.rank()

def main():
    int64 total = 0
    int64 i = 0
    while i < 60000:
        # Three fresh boxes per iteration (180000 total), each dispatched and
        # then dropped — stressing allocation and collection of tier-1 boxes.
        total = total + rank_of(Color.Red)
        total = total + rank_of(Color.Green)
        total = total + rank_of(Color.Blue)
        i = i + 1
    print(total)  # 60000 * (1 + 2 + 3) = 360000
