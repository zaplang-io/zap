# A callable type exported as a module `const` (#1228): usable as a value and
# as a callable from an importing module via `wkday.MO(...)`.
struct Weekday:
    int64 day
    int64 occurrence

    def __call__(self, int64 n = 1) -> Weekday:
        return Weekday(self.day, n)

const Weekday MO = Weekday(0, 1)
