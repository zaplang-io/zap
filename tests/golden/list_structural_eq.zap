# Golden fixture: structural list equality (#1239). `==` / `!=` on two lists
# compares them element-wise, order- and length-sensitive — Python's value
# semantics for sequences — while `is` / `is not` keep reference identity.
#
# Before the fix, `l1 == l2` fell through to the generic comparison path, which
# compared the two list *headers* by pointer: two separately allocated lists with
# equal contents reported unequal (the otel#9 failure). This fixture is the
# intent documentation: it builds separately allocated lists (via a copy helper,
# so the pointers genuinely differ) and asserts value equality, then exercises
# differing order, differing length, aliasing, and `!=`, across int64, float64,
# str, and bool elements. `#1241` (iterate-then-append on list[bool]) is avoided
# for bool by comparing literals directly rather than copying.

# Build a fresh int list so the result is a distinct allocation from its source.
def copy_ints(list[int64] source) -> list[int64]:
    list[int64] out = []
    for v in source:
        out.append(v)
    return out

def copy_floats(list[float64] source) -> list[float64]:
    list[float64] out = []
    for v in source:
        out.append(v)
    return out

def copy_strs(list[str] source) -> list[str]:
    list[str] out = []
    for v in source:
        out.append(v)
    return out

def main():
    # --- int64: separately allocated, equal contents ---
    list[int64] isrc = [1, 2, 3]
    auto icopy = copy_ints(isrc)
    print(str(icopy is not isrc))  # True: distinct allocations
    print(str(icopy == isrc))  # True: equal element-wise
    print(str(icopy == [1, 2, 3]))  # True: equal to a fresh literal
    print(str(icopy != isrc))  # False

    # --- int64: differing order and length are not equal ---
    print(str([1, 2, 3] == [3, 2, 1]))  # False: order-sensitive
    print(str([1, 2, 3] != [3, 2, 1]))  # True
    print(str([1, 2, 3] == [1, 2]))  # False: length-sensitive
    print(str([1, 2] != [1, 2, 3]))  # True
    print(str([1, 2, 3] == [1, 2, 4]))  # False: one element differs

    # --- empty lists compare equal ---
    list[int64] e1 = []
    list[int64] e2 = []
    print(str(e1 == e2))  # True
    print(str(e1 is not e2))  # True: still distinct allocations

    # --- aliasing: same allocation is both == and `is` ---
    auto alias = icopy
    print(str(alias is icopy))  # True: same allocation
    print(str(alias == icopy))  # True

    # --- float64: the element type observed in #1239 (otel#9) ---
    list[float64] fsrc = [0.0, 1.0]
    auto fcopy = copy_floats(fsrc)
    print(str(fcopy is not fsrc))  # True
    print(str(fcopy == fsrc))  # True
    print(str(fcopy == [0.0, 1.0]))  # True
    print(str([0.0, 1.0] == [1.0, 0.0]))  # False: order
    print(str([0.0, 1.0] != [0.0, 2.0]))  # True

    # --- str: reference elements compared by contents, not pointer ---
    list[str] ssrc = ["a", "b"]
    auto scopy = copy_strs(ssrc)
    print(str(scopy is not ssrc))  # True
    print(str(scopy == ssrc))  # True
    print(str(scopy == ["a", "b"]))  # True
    print(str(["a", "b"] == ["b", "a"]))  # False: order
    print(str(["a", "b"] != ["a"]))  # True: length

    # --- bool: literals only (avoids #1241's iterate+append) ---
    print(str([True, False] == [True, False]))  # True
    print(str([True, False] != [False, True]))  # True: order
    print(str([True, False] == [True, True]))  # False

    print("done")
