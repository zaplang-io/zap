# Golden: a callable module-level `const` dispatches to its type's `__call__`
# (#1228, D-0047, #52). `MO(args)` where `MO` is a module global of a type that
# defines `__call__` desugars to `MO.__call__(args)` -- with an explicit
# argument, an omitted default argument (the dateutil `MO()` form), and a
# keyword argument -- while the plain-value use (`MO.day`) is unchanged.
struct Weekday:
    int64 day
    int64 occurrence

    def __call__(self, int64 n = 1) -> Weekday:
        return Weekday(self.day, n)

const Weekday MO = Weekday(0, 1)
const Weekday FR = Weekday(4, 1)

def main():
    # Plain-value use is unchanged.
    print(MO.day)
    print(MO.occurrence)
    # Call with an explicit argument.
    auto last = MO(-1)
    print(last.day)
    print(last.occurrence)
    # Call relying on the default argument.
    auto dflt = FR()
    print(dflt.day)
    print(dflt.occurrence)
    # Keyword argument.
    auto kw = MO(n=2)
    print(kw.occurrence)
