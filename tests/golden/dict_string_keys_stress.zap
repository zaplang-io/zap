# Golden fixture (#837): string-keyed dict at a scale that forces many
# power-of-two resizes, exercising the cached-key-hash probe/resize path (lever
# 1) together with the one-object string keys (lever 2). Keys are built with an
# f-string `f"k{i}"` — the exact shape of the microbenchmark in #837 — and probed
# both as f-strings and as `"k" + str(i)` concats, so a key inserted one way must
# be found the other (same bytes => equal hash and equal compare). Only counts,
# sums and point lookups are printed, so the golden is independent of the dict's
# (unspecified) iteration order.
def main():
    dict[str, int64] d = {}
    auto n = 3000

    # Insert k0..k2999 -> i. Grows the table 8 -> 16 -> ... across ~9 resizes;
    # every resize rehashes from each entry's cached hash (no FNV recompute).
    for i in range(0, n):
        d[f"k{i}"] = i
    print(len(d))  # 3000

    # Sum via f-string lookups: 0 + 1 + ... + 2999 = 4498500.
    int64 s1 = 0
    for i in range(0, n):
        s1 = s1 + d[f"k{i}"]
    print(s1)  # 4498500

    # Overwrite every value in place (hits the existing-key path on set).
    for i in range(0, n):
        d[f"k{i}"] = d[f"k{i}"] + 1
    int64 s2 = 0
    for i in range(0, n):
        s2 = s2 + d[f"k{i}"]
    print(s2)  # 4501500  (4498500 + 3000)

    # Delete k0..k999 (tombstones), leaving i in 1000..2999 with value i+1.
    for i in range(0, 1000):
        del d[f"k{i}"]
    print(len(d))  # 2000

    # Membership + missing-key default across the tombstones.
    print("k0" in d)  # False
    print("k1500" in d)  # True
    print(d.get("k0", -1))  # -1
    print(d["k2999"])  # 3000

    # Cross-construction key equality: a key built by concat finds the f-string
    # key (value i+1 = 1501), proving the hash-word gate + byte compare agree.
    print(d["k" + str(1500)])  # 1501

    # Sum of the 2000 survivors: sum_{i=1000..2999}(i+1) = 3999000 + 2000.
    int64 s3 = 0
    for i in range(1000, n):
        s3 = s3 + d[f"k{i}"]
    print(s3)  # 4001000
