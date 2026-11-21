# Golden: reflected binary operators execute end-to-end (#1201, epic #1189).
#
# Host tests (the crates/sema reflected cases and crates/ir/tests/
# reflected_ops_lower.rs) prove reflected operators type-check and lower to the
# reflected call; this fixture RUNS them and asserts the produced value — the
# arithmetic set over a value-struct left (#1180/#1194), the scalar-left case,
# reflected bitwise/shift with a built-in left, reflected comparison (#1198),
# and a reflected dunder that RAISES, caught by try/except (the throwing_call /
# invoke path, so the unwind is catchable).

value struct Stamp:
    int64 n

# A package-style "right" operand that supplies reflected handlers for a left it
# does not own (the datetime-utils `dt + rd` shape, modeled with local structs):
# `Stamp` defines no `__add__`/`__sub__`, so `s + d` / `s - d` fall through to
# `d.__radd__(s)` / `d.__rsub__(s)`.
struct Delta:
    int64 by

    def __radd__(self, Stamp s) -> Stamp:
        return Stamp(n=s.n + self.by)

    def __rsub__(self, Stamp s) -> Stamp:
        return Stamp(n=s.n - self.by)

# Scalar-left multiply: `k * v` has a built-in (f64) left, so it dispatches to
# `v.__rmul__(k)`.
value struct Vec:
    f64 x

    def __rmul__(self, f64 k) -> Vec:
        return Vec(x=self.x * k)

# Reflected bitwise / shift with a built-in (int64) left operand.
struct Mask:
    int64 bits

    def __rand__(self, int64 n) -> int64:
        return n & self.bits

    def __rlshift__(self, int64 n) -> int64:
        return n << self.bits

# Reflected comparison (#1198): `__rcmp__` returns the sign of `left <=> self`,
# so the relationals derive like the forward single-method `__cmp__`; `__req__`
# handles `==` / `!=`.
struct Money:
    int64 cents

    def __rcmp__(self, int64 n) -> int64:
        if n < self.cents:
            return -1
        if n > self.cents:
            return 1
        return 0

    def __req__(self, int64 n) -> bool:
        return n == self.cents

# A reflected dunder that raises: the operator lowers the reflected call through
# `throwing_call` (invoke), so the raise unwinds and is catchable.
struct Guard:
    int64 limit

    def __radd__(self, Stamp s) -> Stamp:
        if s.n > self.limit:
            raise ValueError("over limit")
        return Stamp(n=s.n + 1)

def main():
    Stamp s = Stamp(n=10)
    Delta d = Delta(by=3)
    Stamp a = s + d
    Stamp b = s - d
    print(a.n)
    print(b.n)

    Vec v = 2.0 * Vec(x=4.0)
    print(v.x)

    print(12 & Mask(bits=10))
    print(1 << Mask(bits=4))

    print(3 < Money(cents=5))
    print(7 < Money(cents=5))
    print(5 == Money(cents=5))
    print(4 == Money(cents=5))

    Stamp ok = Stamp(n=20) + Guard(limit=50)
    print(ok.n)
    try:
        Stamp over = Stamp(n=100) + Guard(limit=50)
        print(over.n)
    except ValueError as e:
        print("caught", e)
