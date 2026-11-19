# Golden fixture: global regex match `=~ m/.../g` yields an *iterable* of match
# objects (#1078, D-0010 — Python `re.finditer` semantics). Exercises ordered,
# non-overlapping matches, an empty iteration when nothing matches, zero-length
# matches making forward progress on both ASCII and multibyte UTF-8 input, and
# numbered + named capture access on each yielded match. The single-match form
# and `s///g` substitution stay covered by regex.zap (unchanged).
def main():
    # Ordered, non-overlapping matches; `group(0)` is each whole match.
    for mt in "abc def" =~ m/[a-z]+/g:
        print(mt.group(0))

    print("--")

    # No match: the loop body never runs (empty iteration).
    auto n = 0
    for mt in "xyz" =~ m/[0-9]+/g:
        n = n + 1
    print(n)

    print("--")

    # Zero-length matches (ASCII): `a*` on "xay" — the empty matches make
    # forward progress (advance one code point) so the iterator terminates.
    # re.finditer yields 4 spans: (0,0) (1,2) (2,2) (3,3).
    auto c = 0
    for mt in "xay" =~ m/a*/g:
        c = c + 1
    print(c)

    print("--")

    # Zero-length matches on multibyte UTF-8: "é" is two bytes, so progress must
    # advance by a full code point, not one byte. Code-point count matches
    # CPython's `re.finditer('a*', 'éa')` -> 3 spans.
    auto u = 0
    for mt in "éa" =~ m/a*/g:
        u = u + 1
    print(u)

    print("--")

    # Numbered capture groups are accessible on each yielded match.
    for mt in "a1 b2" =~ m/([a-z])([0-9])/g:
        auto g1 = mt.group(1)
        auto g2 = mt.group(2)
        print(g1)
        print(g2)

    print("--")

    # Named capture groups are accessible on each yielded match.
    for mt in "x=1;y=2" =~ m/(?<k>[a-z])=(?<v>[0-9])/g:
        auto k = mt.group("k")
        auto v = mt.group("v")
        print(k)
        print(v)
