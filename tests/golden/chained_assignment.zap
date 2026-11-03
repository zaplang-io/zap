# Golden fixture: chained assignment `a = b = c = v` (#594). The right-hand side
# is evaluated EXACTLY ONCE and assigned to every target, left to right (Python
# semantics). Targets may be names, list/dict subscripts, or struct fields — the
# one evaluated value flows into each. `next_val()` prints once, proving the RHS
# is not re-evaluated per target.
struct Point:
    int64 x
    int64 y

def next_val() -> int64:
    print("called once")
    return 7

def main():
    int64 a = 0
    int64 b = 0
    int64 c = 0
    a = b = c = next_val()
    print(a)
    print(b)
    print(c)

    list[int64] xs = [1, 2, 3]
    xs[0] = xs[2] = 42
    print(xs[0])
    print(xs[1])
    print(xs[2])

    dict[str, int64] d = {"p": 0, "q": 0}
    d["p"] = d["q"] = 5
    print(d["p"])
    print(d["q"])

    Point pt = Point(x=0, y=0)
    pt.x = pt.y = 9
    print(pt.x)
    print(pt.y)
