# Golden fixture: comprehensions iterating a `dict` directly (#1154).
#
# Iterating a dict yields its keys (Python semantics, #299), exactly like the
# statement `for k in d` loop. This exercises every comprehension form over a
# dict — list, set, generator, and a dict comprehension whose source clause is
# a dict — across a str-keyed and an int-keyed dict (two `zap_dict_keys_*`
# suffixes), with a filter, and nested inside an `Optional.Some(...)` (the
# shape that surfaced the gap). Dict iteration order is UNSPECIFIED (#1066), so
# every observation here is order-independent: counts, and sums over the keys.
def main():
    dict[str, int64] d = {"a": 1, "b": 2, "c": 3}

    # List comprehension over the dict's keys: 3 keys.
    list[str] ks = [k for k in d]
    print(len(ks))

    # Set comprehension over the same keys: 3 distinct keys.
    set[str] kset = {k for k in d}
    print(len(kset))

    # Generator expression over the dict, consumed by a `for` loop.
    auto gcount = 0
    for k in (k for k in d):
        gcount = gcount + 1
    print(gcount)

    # Dict comprehension whose source clause iterates the dict: map each key to
    # the dict's value for that key, then sum the values -> 1 + 2 + 3 = 6.
    dict[str, int64] rebuilt = {k: d[k] for k in d}
    auto vsum = 0
    for k in rebuilt.keys():
        vsum = vsum + rebuilt[k]
    print(vsum)

    # Int-keyed dict: a different key suffix. Sum the keys -> 10 + 20 + 30 = 60.
    dict[int64, str] nums = {10: "x", 20: "y", 30: "z"}
    list[int64] nkeys = [k for k in nums]
    auto ksum = 0
    for k in nkeys:
        ksum = ksum + k
    print(ksum)

    # Filter on the key clause: keys > 10 -> {20, 30}, count 2.
    list[int64] big = [k for k in nums if k > 10]
    print(len(big))

    # Nested inside Optional.Some(...): the comprehension's element type lands
    # in the payload position.
    Optional[list[str]] maybe = Optional.Some([k for k in d])
    print(maybe.is_some())
    print(len(maybe.unwrap()))

    # Empty dict: the materialized key list is empty.
    dict[str, int64] empty = {}
    list[str] none_keys = [k for k in empty]
    print(len(none_keys))
