# Golden fixture: variadic tuple[T] as a dict key / set member (#1474, hashing).
# A tuple[T] hashes by content (fold of element hashes) and compares by value, so
# a same-content tuple built separately hits the same entry. Only fixed labels /
# lengths / booleans are printed (dict/set order is unspecified, #1066).
def main():
    # --- dict with tuple[int64] keys ---
    dict[tuple[int64], str] d = dict[tuple[int64], str]()
    d[tuple([1, 2])] = "a"
    d[tuple([3, 4])] = "b"
    print(d[tuple([1, 2])])  # a   (same-content key, different object)
    print(len(d))  # 2
    print(tuple([1, 2]) in d)  # True
    print(tuple([9, 9]) in d)  # False

    # --- set of tuples: dedup by content ---
    auto s = set([tuple([1, 2]), tuple([3, 4]), tuple([1, 2])])
    print(len(s))  # 2
    print(tuple([3, 4]) in s)  # True
    print(tuple([5, 6]) in s)  # False

    # --- str-element tuple key ---
    dict[tuple[str], int64] m = dict[tuple[str], int64]()
    m[tuple(["x", "y"])] = 7
    print(m[tuple(["x", "y"])])  # 7
