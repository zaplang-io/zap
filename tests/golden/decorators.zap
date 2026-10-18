# Golden fixture: decorators & annotations (ticket #16, D-0036).
#
# `@derive(Eq, Repr)` is a built-in compile-time decorator: it generates the
# `eq` and `repr` methods as real, type-checked code (zero runtime cost).
# `@Table` is a user annotation; it leaves no generated code but is recorded in
# the runtime-reflection registry, queried below with `reflect_has` /
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
    print(a.repr())
    print(c.repr())
    if a.eq(b):
        print("a == b")
    if not a.eq(c):
        print("a != c")
    if reflect_has("User", "Table"):
        print("User is annotated @Table")
    if not reflect_has("User", "Nope"):
        print("User has no @Nope")
    print(reflect_names("User"))
