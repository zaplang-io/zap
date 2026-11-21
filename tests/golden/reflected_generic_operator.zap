# Golden: generic operator dunders dispatch through operator syntax (#1227,
# D-0047, #1180). A generic reflected dunder (`def __radd__[T]`) resolves
# through `a + b` operator syntax, instantiated with the concrete left-operand
# type -- covered for both a `Date` and a `DateTime` left operand and for both
# `__radd__` and `__rsub__`. A generic FORWARD dunder dispatches too. A
# non-generic reflected dunder and left-forward-wins precedence are preserved.
import datetime

# Reflected generic dunder: returns the left operand unchanged (identity), so
# the result is the concrete left-operand type `T` (Date or DateTime).
struct Shift:
    int64 days

    def __radd__[T](self, T dt) -> T:
        return dt

    def __rsub__[T](self, T dt) -> T:
        return dt

# Forward generic dunder on the left operand.
struct Counter:
    int64 n

    def __add__[T](self, T other) -> int64:
        return self.n

# Non-generic reflected dunder: must still dispatch via operator syntax.
struct Scale:
    int64 k

    def __rmul__(self, int64 x) -> int64:
        return x * self.k

# Non-generic forward dunder: `Money + Money` dispatches forward (left wins).
struct Money:
    int64 cents

    def __add__(self, Money other) -> int64:
        return self.cents + other.cents

def main():
    auto d = datetime.date(2024, 3, 14)
    auto ts = datetime.parse("2024-03-14T09:30:00")
    # Reflected generic: two distinct left-operand types, two operators.
    auto a = d + Shift(1)
    auto b = ts + Shift(2)
    auto c = d - Shift(3)
    auto e = ts - Shift(4)
    assert a.day == d.day
    assert b.day == ts.day
    assert c.day == d.day
    assert e.day == ts.day
    # Forward generic dunder, instantiated per argument type.
    print(Counter(7) + "x")
    print(Counter(7) + 99)
    # Non-generic reflected dunder preserved.
    print(3 * Scale(10))
    # Left-forward-wins precedence preserved.
    print(Money(150) + Money(50))
    print("ok")
