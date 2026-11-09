# Golden fixture: value structs AUTO-derive Eq + Hash with NO `@derive`
# (D-0139/#808). A value struct is copied on insert and has no identity
# (D-0023/D-0051), so by-value equality is its only coherent meaning; the compiler
# synthesizes Eq+Hash iff every field is hashable. Covers a scalar value struct as
# a dict key + set member (equal keys collide, overwrite, de-dup), the auto-derived
# `==`, a nested value aggregate (a value struct field), and a float field — the
# `-0.0`/`+0.0` collision inherited from the shared #803 key word.
value struct Point:
    int64 x
    int64 y


value struct Inner:
    int64 a


value struct Outer:
    Inner i
    int64 b


value struct FPoint:
    float64 v


def main():
    # Auto-derived Eq/Hash as a dict key — no @derive needed.
    dict[Point, int64] d = {}
    d[Point(x=1, y=2)] = 100
    print(d[Point(x=1, y=2)])  # 100  (equal keys collide)
    d[Point(x=1, y=2)] = 200  # overwrite the same slot
    print(d[Point(x=1, y=2)])  # 200
    print(len(d))  # 1

    # Auto-derived `==`.
    print(Point(x=1, y=2) == Point(x=1, y=2))  # True
    print(Point(x=1, y=2) == Point(x=9, y=9))  # False

    # Set membership de-dups equal value structs.
    set[Point] s = set[Point]()
    s.add(Point(x=3, y=4))
    s.add(Point(x=3, y=4))
    s.add(Point(x=5, y=6))
    print(len(s))  # 2

    # A value aggregate of value aggregates auto-derives recursively.
    dict[Outer, str] od = {}
    od[Outer(i=Inner(a=1), b=2)] = "hi"
    print(od[Outer(i=Inner(a=1), b=2)])  # hi
    print(len(od))  # 1

    # Float field: `-0.0` and `+0.0` collide (canonicalized, #803).
    dict[FPoint, int64] fd = {}
    fd[FPoint(v=0.0)] = 7
    print(fd[FPoint(v=-0.0)])  # 7
    print(len(fd))  # 1
