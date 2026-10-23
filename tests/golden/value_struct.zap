# Golden fixture: value structs (value struct, #65, D-0051).
# A `value struct` is a value type: it is COPIED on assignment, argument passing,
# and return (value semantics, no shared identity). So mutating a copy — or a
# callee's by-value parameter — leaves the caller's value untouched, unlike a
# plain (reference) struct (D-0023).
value struct Vec2:
    f64 x
    f64 y

    def len2(self) -> f64:
        return self.x * self.x + self.y * self.y

# Takes its Vec2 by value: the mutation below acts on the callee's own copy.
def bump(Vec2 v) -> f64:
    v.x = 100.0
    return v.x

def main():
    auto a = Vec2(x=3.0, y=4.0)
    auto b = a  # value copy (D-0051)
    b.x = 30.0  # mutate the copy only
    b.y = 40.0
    print(a.x)  # 3.0  — original unchanged by the copy's mutation
    print(b.x)  # 30.0
    print(bump(a))  # 100.0 — the callee's copy
    print(a.x)  # 3.0  — caller's value unchanged after pass-by-value
    print(a.len2())  # 25.0 (9 + 16)
    print(b.len2())  # 2500.0 (900 + 1600)
