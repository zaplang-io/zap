# #1455: a method whose declared return type is `Self` is typed as the enclosing
# type at the call site, so its result can be used — field-accessed, chained —
# like any value of that type. Before the fix the `Self` return resolved to a
# non-struct in codegen, so a field access on the result failed with
# `error[ZAP6001]: field access on a non-struct value`. Covers a hand-written
# method returning `Self` (bare and chained) and a `Self`-typed parameter.
struct P:
    int64 x
    int64 y

    def clone(self) -> Self:
        return self

    def max_x(self, Self other) -> Self:
        if other.x > self.x:
            return other
        return self

def main():
    auto p = P(3, 4)
    auto q = p.clone()
    print(q.x)
    print(q.y)
    # chained: the result of a `Self`-returning method is itself usable.
    print(p.clone().x)
    # a `Self`-typed parameter resolves too.
    auto r = P(9, 0)
    print(p.max_x(r).x)
