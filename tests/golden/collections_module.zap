# Golden fixture: the `collections` module (#972, epic #966) — the Python
# `collections` conveniences, each a generic prelude struct over the built-in
# `dict`/`list` (#812), pure prelude Zap with no runtime C.
#
#   * Counter[K]     — a multiset/tally; missing keys read 0; `most_common`,
#                      `elements`, `total`, add/subtract, and `+`/`-` (which
#                      drop non-positive counts).
#   * defaultdict[K,V] — autovivifies a missing key from a `fn() -> V` factory,
#                      enabling the `d[key].append(x)` grouping idiom; `in` /
#                      `get` are read-only and never create the key.
#   * OrderedDict[K,V] — an insertion-ordered map (a real gap: built-in `dict`
#                      order is unspecified/seeded, D-0235), with `move_to_end`
#                      and `delete`.
#
# Determinism note (D-0235): the backing `dict` has no defined iteration order,
# so this fixture asserts only order-independent facts — counts, totals, a
# `most_common` over DISTINCT counts, and `elements` via its length — plus the
# OrderedDict surface, whose order IS defined (insertion order). It never prints
# raw `Counter`/`defaultdict` key order.
import collections

def mk_list() -> list[int64]:
    list[int64] e = []
    return e

def zero() -> int64:
    return 0

def main():
    # --- Counter: construct from an iterable; missing key reads 0 ---
    collections.Counter[str] c = collections.Counter(["a", "b", "a", "c", "a", "b"])
    print("count", c["a"], c["b"], c["c"], c["z"])
    print("len_total", len(c), c.total())
    print("contains", "a" in c, "z" in c)

    # --- most_common: distinct counts => fully deterministic order ---
    auto mc = c.most_common(2)
    print("mc2", mc[0][0], mc[0][1], mc[1][0], mc[1][1])
    auto mcall = c.most_common()
    print("mcall", len(mcall), mcall[2][0], mcall[2][1])

    # --- elements(): 3 a + 2 b + 1 c = 6 (order unspecified, so assert len) ---
    print("elements", len(c.elements()))

    # --- add / subtract: non-positive counts are KEPT (Python subtract) ---
    c.add("a")
    c.add("d", 5)
    c.subtract("b", 10)
    print("after", c["a"], c["d"], c["b"])

    # --- arithmetic + / - : non-positive results are DROPPED (Python) ---
    collections.Counter[str] c1 = collections.Counter(["x", "x", "y"])
    collections.Counter[str] c2 = collections.Counter(["x", "z"])
    auto added = c1 + c2
    print("added", added["x"], added["y"], added["z"], len(added))
    auto subbed = c1 - c2
    print("subbed", subbed["x"], subbed["y"], subbed["z"], len(subbed))

    # --- defaultdict(list): the grouping idiom d[key].append(x) ---
    collections.defaultdict[str, list[int64]] groups = collections.defaultdict(mk_list)
    groups["apples"].append(1)
    groups["apples"].append(2)
    groups["pears"].append(3)
    print("groups", len(groups["apples"]), len(groups["pears"]), len(groups))
    print("groups_in", "apples" in groups, "grapes" in groups)
    # read-only membership must NOT create the key
    print("groups_len_stable", len(groups))
    # get(fallback) is read-only too
    auto empty = mk_list()
    print("groups_get", len(groups.get("grapes", empty)), len(groups))

    # --- defaultdict(int): the accumulate idiom ---
    collections.defaultdict[str, int64] tally = collections.defaultdict(zero)
    tally["hits"] = tally["hits"] + 1
    tally["hits"] = tally["hits"] + 1
    tally["miss"] = tally["miss"] + 1
    print("tally", tally["hits"], tally["miss"], len(tally))

    # --- OrderedDict: insertion order is DEFINED and deterministic ---
    collections.OrderedDict[str, int64] od = collections.OrderedDict()
    od["one"] = 1
    od["two"] = 2
    od["three"] = 3
    od["two"] = 20
    print("od_keys", od.keys()[0], od.keys()[1], od.keys()[2])
    print("od_vals", od.values()[0], od.values()[1], od.values()[2])
    print("od_len", len(od))
    od.move_to_end("one")
    print("od_moved", od.keys()[0], od.keys()[1], od.keys()[2])
    od.delete("three")
    print("od_del", len(od), od.keys()[0], od.keys()[1])
    print("od_get", od.get("missing", 0 - 1), od.get("one", 0))
    print("od_in", "two" in od, "three" in od)
