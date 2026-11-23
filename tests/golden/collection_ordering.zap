# Golden fixture: ordering comparisons on collections (#1334), the ordering
# sibling of the equality fix (#1328). `set` has Python's subset/superset partial
# order; `deque` has Python's lexicographic ordering. `dict`, `heap`, and `oset`
# are not orderable and are rejected at compile time, so they cannot appear here
# (covered by the sema/lowering tests).
#
# Output is strictly boolean. Set membership order is unspecified/seed-dependent,
# but subset/superset results are order-independent; deque ordering is
# deterministic. Neither depends on iteration order.

def main():
    # --- set subset / superset: a is a proper subset of b ---
    set[int64] a = {1, 2}
    set[int64] b = {1, 2, 3}
    print(str(a <= b))  # True: subset
    print(str(a < b))  # True: proper subset
    print(str(a >= b))  # False
    print(str(a > b))  # False
    print(str(b >= a))  # True: superset
    print(str(b > a))  # True: proper superset
    print(str(b <= a))  # False
    print(str(b < a))  # False

    # --- equal sets (built reordered): subset both ways, never proper ---
    set[int64] c = {2, 1}
    print(str(a <= c))  # True
    print(str(a < c))  # False: equal, not proper
    print(str(a >= c))  # True
    print(str(a > c))  # False

    # --- incomparable sets: neither is a subset of the other ---
    set[int64] d = {2, 3}
    print(str(a <= d))  # False
    print(str(d <= a))  # False
    print(str(a < d))  # False
    print(str(a > d))  # False

    # --- empty set is a subset of everything ---
    set[int64] e = set[int64]()
    set[int64] e2 = set[int64]()
    print(str(e <= a))  # True
    print(str(e < a))  # True: proper (a is non-empty)
    print(str(a <= e))  # False
    print(str(a < e))  # False
    print(str(e <= e2))  # True
    print(str(e < e2))  # False: equal (both empty)

    # --- str members ---
    print(str({"a"} <= {"a", "b"}))  # True
    print(str({"a", "b"} > {"a"}))  # True

    # --- deque lexicographic ordering ---
    deque[int64] qa = deque[int64]()
    qa.append(1)
    qa.append(2)
    qa.append(3)
    deque[int64] qb = deque[int64]()
    qb.append(1)
    qb.append(2)
    qb.append(3)
    print(str(qa < qb))  # False: equal
    print(str(qa <= qb))  # True
    print(str(qa >= qb))  # True
    print(str(qa > qb))  # False

    # --- first differing element decides ---
    deque[int64] qc = deque[int64]()
    qc.append(1)
    qc.append(2)
    qc.append(4)
    print(str(qa < qc))  # True: 3 < 4 at index 2
    print(str(qa > qc))  # False
    print(str(qa <= qc))  # True
    print(str(qc > qa))  # True

    # --- a prefix is smaller than its extension ---
    deque[int64] qp = deque[int64]()
    qp.append(1)
    qp.append(2)
    print(str(qp < qa))  # True: [1,2] is a prefix of [1,2,3]
    print(str(qp <= qa))  # True
    print(str(qp > qa))  # False
    print(str(qa > qp))  # True

    # --- the first element dominates later ones ---
    deque[int64] qx = deque[int64]()
    qx.append(2)
    deque[int64] qy = deque[int64]()
    qy.append(1)
    qy.append(9)
    qy.append(9)
    print(str(qx < qy))  # False: 2 > 1 at index 0
    print(str(qx > qy))  # True

    # --- empty deques ---
    deque[int64] qe = deque[int64]()
    deque[int64] qf = deque[int64]()
    print(str(qe < qf))  # False: equal (both empty)
    print(str(qe <= qf))  # True
    deque[int64] qone = deque[int64]()
    qone.append(1)
    print(str(qe < qone))  # True: empty is a prefix of [1]
    print(str(qone > qe))  # True

    # --- str deque ---
    deque[str] sa = deque[str]()
    sa.append("a")
    sa.append("b")
    deque[str] sc = deque[str]()
    sc.append("a")
    sc.append("c")
    print(str(sa < sc))  # True: "b" < "c" at index 1
    print(str(sc > sa))  # True

    print("done")
