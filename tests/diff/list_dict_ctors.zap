# diff: match
# list(iterable) and dict(pairs/copy/kwargs) match CPython (#1460) for the
# deterministic cases: list over ordered sources, and dict observed by key
# lookup and len. dict repr order is unspecified (#1066), so it is never printed.
def main():
    print(list(range(3)))
    print(list("abc"))
    print(list([10, 20, 30]))
    print(len(list(range(0))))
    auto d = dict([(1, "a"), (2, "b")])
    print(d[1])
    print(d[2])
    print(len(d))
    auto c = dict(d)
    print(c[2])
    print(len(c))
    auto k = dict(x=1, y=2)
    print(k["x"])
    print(k["y"])
    print(len(k))
