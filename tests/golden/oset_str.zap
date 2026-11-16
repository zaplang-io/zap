# Golden fixture: an insertion-ordered set of strings (#983).
#
# `oset[str]` is a `Hashable`-element ordered set: membership is O(1) via the
# backing hash set (str keys hashed by content), iteration follows first-add
# order, and duplicates are ignored. Also exercises the `oset[T](iterable)`
# constructor and `x in s` membership.
def main():
    auto s = oset[str]()
    s.add("foo")
    s.add("bar")
    s.add("foo")
    s.add("baz")
    print(len(s))
    for x in s:
        print(x)
    if "bar" in s:
        print(1)
    else:
        print(0)
    if "qux" in s:
        print(1)
    else:
        print(0)
    print(s.get(0))
    # constructor from an iterable: dedup, keep first-seen order
    auto t = oset[str](["a", "b", "a", "c"])
    print(len(t))
    for y in t:
        print(y)
