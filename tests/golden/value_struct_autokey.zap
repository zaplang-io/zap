# Golden fixture: a value struct AUTO-derives Eq + Hash with NO `@derive`
# (D-0139/#808). A value struct is copied on insert and has no identity
# (D-0023/D-0051), so by-value equality is its only coherent meaning; the compiler
# synthesizes Eq+Hash iff every field is hashable. This pins the runtime of the
# common case — a flat scalar value struct as a dict key + set member (equal keys
# collide, overwrite, de-dup) and the auto-derived `==` — which `value_struct_key`
# exercises with an explicit `@derive`; here it is derived automatically.
# (Nested-aggregate and float-FIELD value-struct keys are covered structurally by
# the IR tests; their runtime goldens are a follow-up — see PR #816.)
value struct Point:
    int64 x
    int64 y


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
