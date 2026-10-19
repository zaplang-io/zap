# Golden fixture: decorators & annotations (ticket #16, D-0036).
#
# `@derive(Eq, Repr)` is a built-in compile-time decorator: it generates the
# `__eq__` and `__repr__` dunders as real, type-checked code (zero runtime cost,
# D-0047). `==`/`!=` desugar to `__eq__`, and `print(x)` renders through
# `__repr__`. `@Table` is a user annotation; it leaves no generated code but is
# recorded in the runtime-reflection registry, queried below with `reflect_has` /
# `reflect_names`.

annotation Table:
    str name = ""

@derive(Eq, Repr)
@Table(name="users")
struct User:
    int64 id
    str name

def main():
    auto a = User(id=1, name="alice")
    auto b = User(id=1, name="alice")
    auto c = User(id=2, name="bob")
    print(a)
    print(c)
    if a == b:
        print("a == b")
    if a != c:
        print("a != c")
    if reflect_has("User", "Table"):
        print("User is annotated @Table")
    if not reflect_has("User", "Nope"):
        print("User has no @Nope")
    print(reflect_names("User"))
