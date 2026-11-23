# Golden fixture: lexicographic list ordering (#1334, list sibling). `< <= > >=`
# on two lists compare element-wise — the first differing element decides (by
# element `<`), and a shorter list that is a prefix of the other sorts first —
# matching Python's sequence ordering. `is` / `is not` keep reference identity.
#
# Before the fix, `a < b` fell through to the generic comparison path, which
# `ptrtoint`-compared the two list headers (a meaningless allocation-address
# compare). This fixture exercises order, length/prefix, the first-element
# dominance rule, empties, and int / float / str / bool element types, plus a
# separately allocated (copy-helper) list to prove ordering is value-based.

def copy_ints(list[int64] source) -> list[int64]:
    list[int64] out = []
    for v in source:
        out.append(v)
    return out

def main():
    # --- equal lists: < / > False, <= / >= True ---
    list[int64] a = [1, 2, 3]
    list[int64] b = [1, 2, 3]
    print(str(a < b))  # False
    print(str(a <= b))  # True
    print(str(a >= b))  # True
    print(str(a > b))  # False

    # --- first differing element decides ---
    list[int64] c = [1, 2, 4]
    print(str(a < c))  # True: 3 < 4 at index 2
    print(str(a > c))  # False
    print(str(a <= c))  # True
    print(str(c > a))  # True

    # --- a prefix sorts before its extension ---
    list[int64] p = [1, 2]
    print(str(p < a))  # True: [1,2] is a prefix of [1,2,3]
    print(str(p <= a))  # True
    print(str(p > a))  # False
    print(str(a > p))  # True

    # --- the first element dominates later ones ---
    list[int64] x = [2]
    list[int64] y = [1, 9, 9]
    print(str(x < y))  # False: 2 > 1 at index 0
    print(str(x > y))  # True

    # --- empty lists ---
    list[int64] e = []
    list[int64] f = []
    print(str(e < f))  # False: equal (both empty)
    print(str(e <= f))  # True
    print(str(e < [1]))  # True: empty is a prefix of [1]
    print(str([1] > e))  # True

    # --- float elements ---
    print(str([1.0, 2.0] < [1.0, 3.0]))  # True
    print(str([1.0, 2.0] < [1.0, 2.0]))  # False
    print(str([1.0, 2.0] <= [1.0, 2.0]))  # True

    # --- str elements compared by contents, not pointer ---
    print(str(["a", "b"] < ["a", "c"]))  # True
    print(str(["a", "b"] < ["a", "b", "c"]))  # True: prefix
    print(str(["b"] > ["a", "z", "z"]))  # True: "b" > "a" at index 0

    # --- bool elements (False < True), restored after the bool zero-extend fix (#1344) ---
    print(str([False, True] < [True, False]))  # True: False < True at index 0
    print(str([True] > [False]))  # True

    # --- separately allocated (copy helper): ordering is value-based ---
    list[int64] isrc = [1, 2, 3]
    auto icopy = copy_ints(isrc)
    print(str(icopy <= isrc))  # True: equal contents
    print(str(icopy < isrc))  # False: equal, not strictly less
    print(str(icopy is not isrc))  # True: distinct allocations

    print("done")
