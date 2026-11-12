# Golden fixture (#840): int-keyed dict at a scale that forces many power-of-two
# resizes, then heavy tombstone churn and slot reuse — the SoA / tag table's
# resize, delete-tombstone, and tombstone-reuse paths. A tag (top-8 hash bits)
# collision between two distinct keys must fall through to the full key compare,
# never a false hit, so every point lookup and every reduced sum below would be
# wrong if the tag discrimination or the resize rehash dropped/aliased a key.
# Only counts, sums, point lookups and membership are printed — independent of
# the dict's unspecified iteration order.
def main():
    dict[int64, int64] d = {}
    auto n = 5000

    # Insert 0..4999 -> i*3 (grows the table 8 -> 16 -> ... across ~10 resizes).
    for i in range(0, n):
        d[i] = i * 3
    print(len(d))  # 5000
    int64 s1 = 0
    for i in range(0, n):
        s1 = s1 + d[i]
    print(s1)  # 3 * (0+..+4999) = 37492500

    # Overwrite every key in place (existing-key path on set).
    for i in range(0, n):
        d[i] = d[i] + 1
    int64 s2 = 0
    for i in range(0, n):
        s2 = s2 + d[i]
    print(s2)  # 37492500 + 5000 = 37497500

    # Delete the first half (2500 tombstones on live probe chains).
    for i in range(0, 2500):
        del d[i]
    print(len(d))  # 2500

    # Membership across the tombstones: only 2500..4999 remain.
    int64 present = 0
    for i in range(0, n):
        if i in d:
            present = present + 1
    print(present)  # 2500

    # Reinsert the deleted half with a new value, reusing tombstone slots.
    for i in range(0, 2500):
        d[i] = 9
    print(len(d))  # 5000

    # Sum after reuse: 2500*9 for 0..2499, plus (i*3+1) for 2500..4999.
    int64 s3 = 0
    for i in range(0, n):
        s3 = s3 + d[i]
    print(s3)  # 2500*9 + sum_{2500..4999}(3i+1) = 22500 + 28123750 = 28146250

    # Missing-key default and membership for a never-inserted key.
    print(d.get(999999, -7))  # -7
    print(n in d)  # False
