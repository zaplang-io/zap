# I#972 (negative): the Python name `Counter` is exposed by TWO modules —
# `collections.Counter` (this issue's multiset, backing prelude struct
# `CounterMap`) and the pre-existing lock-free `concurrent.Counter` (D-0078,
# backing prelude struct `Counter`). They are distinct types backed by distinct
# prelude structs, so the lock-free counter does NOT gain the multiset surface:
# calling `.total()` on it is a compile error. This pins the name-clash
# resolution — the seam sugar keeps `collections.Counter` off the `Counter`
# struct entirely.
import concurrent

def main():
    auto c = concurrent.Counter()
    print(c.total())
