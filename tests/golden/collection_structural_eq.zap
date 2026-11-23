# Golden fixture: structural collection equality (#1328). The dict counterpart to
# the list-equality fix (#1239), widened across the core collections.
#
# Before the fix, `==` / `!=` on dict / set / deque / oset fell through to the
# generic comparison path, which compared the two container *headers* by pointer:
# two separately allocated containers with equal contents reported unequal. This
# fixture is the intent documentation. It builds separately allocated containers
# and asserts value equality, then exercises reordering, differing length,
# differing contents, emptiness, nesting, and identity.
#
# dict / set / oset iteration order is unspecified and seed-dependent (wyhash), so
# this fixture prints ONLY boolean results of ==/!=/is — never a container's repr
# or iteration order. dict and set equality are order-INDEPENDENT; deque and oset
# equality are order-SENSITIVE sequence equality (oset's insertion order is
# observable and is why it is distinct from set). `heap` has no value equality
# (its internal order is implementation-defined), so it is rejected at compile
# time and cannot appear here; that is covered by the sema/lowering tests.

def main():
    # --- dict[str, int64]: equal, built in a different key order ---
    dict[str, int64] left = {"x": 1, "y": 2}
    dict[str, int64] right = {"y": 2, "x": 1}
    print(str(left is not right))  # True: distinct allocations
    print(str(left == right))  # True: equal entries, order-independent
    print(str(left != right))  # False
    print(str(left is left))  # True: identity preserved

    # --- dict: unequal value, missing/extra key, differing length ---
    print(str({"x": 1} == {"x": 2}))  # False: value differs
    print(str({"x": 1, "y": 2} == {"x": 1, "z": 2}))  # False: a key differs
    print(str({"x": 1} == {"x": 1, "y": 2}))  # False: length differs
    print(str({"x": 1} != {"x": 1, "y": 2}))  # True

    # --- dict: empty maps compare equal, still distinct allocations ---
    dict[str, int64] e1 = {}
    dict[str, int64] e2 = {}
    print(str(e1 == e2))  # True
    print(str(e1 is not e2))  # True

    # --- dict[int64, int64]: integer keys, reordered ---
    print(str({1: 10, 2: 20} == {2: 20, 1: 10}))  # True

    # --- dict whose VALUE type supplies its own equality (nested list, #1239) ---
    dict[str, list[int64]] la = {"a": [1, 2], "b": [3]}
    dict[str, list[int64]] lb = {"b": [3], "a": [1, 2]}
    print(str(la == lb))  # True: recurses into structural list equality
    print(str({"a": [1, 2]} == {"a": [1, 9]}))  # False: nested list differs

    # --- dict whose VALUE type is itself a dict (recurse into dict equality) ---
    dict[str, dict[str, int64]] da = {"g": {"x": 1, "y": 2}}
    dict[str, dict[str, int64]] db = {"g": {"y": 2, "x": 1}}
    print(str(da == db))  # True: nested dict equal, order-independent

    # --- otel-style aggregation shape (zaplang-io/otel#3): two measurements built
    # from equal attribute sets (different insertion order) must compare equal, so
    # they aggregate into one stream. ---
    dict[str, str] attrs_a = {"service": "api", "region": "us"}
    dict[str, str] attrs_b = {"region": "us", "service": "api"}
    print(str(attrs_a == attrs_b))  # True: same attribute set -> one stream

    # --- set[int64]: membership-based, order-independent ---
    set[int64] sa = {1, 2, 3}
    set[int64] sb = {3, 2, 1}
    print(str(sa is not sb))  # True
    print(str(sa == sb))  # True: same members
    print(str(sa != sb))  # False
    print(str({1, 2} == {1, 2, 3}))  # False: length differs
    print(str({1, 2, 3} == {1, 2, 4}))  # False: a member differs
    set[int64] se1 = set[int64]()
    set[int64] se2 = set[int64]()
    print(str(se1 == se2))  # True: empty sets equal
    print(str({"a", "b"} == {"b", "a"}))  # True: str members, order-independent

    # --- deque[int64]: order-SENSITIVE, length-sensitive, element-wise ---
    deque[int64] qa = deque[int64]()
    qa.append(1)
    qa.append(2)
    qa.append(3)
    deque[int64] qb = deque[int64]()
    qb.append(1)
    qb.append(2)
    qb.append(3)
    print(str(qa is not qb))  # True: distinct allocations
    print(str(qa == qb))  # True: equal in order
    print(str(qa != qb))  # False
    deque[int64] qrev = deque[int64]()
    qrev.append(3)
    qrev.append(2)
    qrev.append(1)
    print(str(qa == qrev))  # False: order-sensitive
    deque[int64] qshort = deque[int64]()
    qshort.append(1)
    qshort.append(2)
    print(str(qa == qshort))  # False: length differs
    deque[int64] qe1 = deque[int64]()
    deque[int64] qe2 = deque[int64]()
    print(str(qe1 == qe2))  # True: empty deques equal
    auto qalias = qa
    print(str(qalias is qa))  # True: same allocation
    print(str(qalias == qa))  # True

    # --- oset[int64]: order-SENSITIVE sequence equality, deduplicated ---
    oset[int64] oa = oset[int64]([1, 2, 3])
    oset[int64] ob = oset[int64]([1, 2, 3])
    print(str(oa is not ob))  # True
    print(str(oa == ob))  # True: equal in order
    print(str(oa == oset[int64]([3, 2, 1])))  # False: order-sensitive
    print(str(oa == oset[int64]([1, 2])))  # False: length differs
    print(str(oset[int64]([1, 1, 2]) == oset[int64]([1, 2])))  # True: dedup, same order
    oset[int64] oe1 = oset[int64]([])
    oset[int64] oe2 = oset[int64]([])
    print(str(oe1 == oe2))  # True: empty osets equal
    print(str(oset[str](["a", "b"]) == oset[str](["a", "b"])))  # True: str elements

    print("done")
