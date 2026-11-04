# @cache (#666) rejects a reference-typed key at compile time (ZAP5056). A
# `struct` argument would be keyed by identity, not value, so two equal-but-
# distinct `Point`s would never hit — the cache would silently give no benefit
# and be hard to debug. v1 keys must be an integer, `bool`, or `str`; this is a
# clean compile error, not a silent non-memoizing cache. Compile-fail fixture.
struct Point:
    int64 x
    int64 y

@cache
def norm(Point p) -> int64:
    return p.x * p.x + p.y * p.y

def main():
    print(norm(Point(3, 4)))
