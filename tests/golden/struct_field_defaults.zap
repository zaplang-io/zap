# Golden fixture (#461, D-0038): a struct field with a declared default is
# initialized even when the caller omits it at construction. `Box()` must
# behave exactly like `Box(xs=[])`; before the fix the omitted `list` default
# was left null and `.append` SIGSEGV'd (and other pointer defaults read null
# silently). Covers scalar, str, list, and nested-struct defaults, with
# fully- and partially-omitted construction, plus mutable-default freshness.

struct Point:
    int64 x = 1
    int64 y = 2

struct Config:
    int64 n = 7
    str label = "def"
    list[int64] xs = []
    Point origin = Point(x=3, y=4)

def dump(Config c):
    print(c.n)
    print(c.label)
    print(len(c.xs))
    print(c.origin.x)
    print(c.origin.y)

def main():
    # Fully omitted: every field comes from its default.
    auto a = Config()
    a.xs.append(10)  # would SIGSEGV on a null list before #461
    dump(a)  # 7 / def / 1 / 3 / 4

    # Partially omitted: n given, the rest defaulted. The mutable `[]` default
    # is a fresh list, independent of `a.xs`.
    auto b = Config(n=100)
    b.xs.append(20)
    b.xs.append(21)
    dump(b)  # 100 / def / 2 / 3 / 4
    print(len(a.xs))  # still 1 — a.xs and b.xs are distinct objects

    # Explicit form of the fully-omitted case: identical output.
    auto c = Config(n=7, label="def", xs=[], origin=Point(x=3, y=4))
    c.xs.append(10)
    dump(c)  # 7 / def / 1 / 3 / 4

    # A struct built from scalar defaults only.
    auto p = Point()
    print(p.x)  # 1
    print(p.y)  # 2
