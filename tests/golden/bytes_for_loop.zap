# Golden fixture: iterating a `bytes` value in a `for` loop (ticket #1349).
#
# `for b in <bytes>` yields each raw byte as an unsigned int64 in 0..=255, in
# order, byte-wise (not UTF-8-decoded). This exercises: the issue's b"UTC" sum,
# unsigned 0x00/0xFF (never sign-extended to negative, cf. the #1344 bool
# regression), empty bytes (zero iterations), order preservation, using the
# loop variable as an int (arithmetic + list index), nesting, and agreement
# with byte indexing. End-to-end under `zap run`; the native `zap test` path is
# covered by `test_bytes_iteration_1349`.
def main():
    # The issue's exact reduction: 85 + 84 + 67 == 236.
    auto data = b"UTC"
    int64 total = 0
    for byte in data:
        total += byte
    print(total)  # 236

    # Order is preserved: the bytes come out U, T, C.
    for b in data:
        print(b)  # 85, then 84, then 67

    # Unsigned 0..=255: 0x00 and 0xFF read back as 0 and 255, and the high
    # bytes 128/255 are NOT sign-extended to negative (they stay >= 0).
    auto edges = bytes([0, 127, 128, 255])
    for b in edges:
        print(b)  # 0, 127, 128, 255
    bool all_unsigned = True
    for b in edges:
        if b < 0:
            all_unsigned = False
    print(all_unsigned)  # True

    # Empty bytes: zero iterations.
    int64 empties = 0
    for b in bytes():
        empties += 1
    print(empties)  # 0

    # The loop variable is an int: usable in arithmetic and as a list index.
    list[int64] seen = []
    for b in b"abc":
        seen.append(b + 1)
    print(seen[0])  # 98
    print(seen[1])  # 99
    print(seen[2])  # 100

    # Iteration agrees with byte indexing (same values, same order).
    auto msg = b"hello"
    int64 iter_sum = 0
    for b in msg:
        iter_sum += b
    int64 index_sum = 0
    for i in range(0, len(msg)):
        index_sum += msg[i]
    print(iter_sum == index_sum)  # True

    # Nested bytes loops: each binds its own int64 byte.
    int64 pairs = 0
    for a in b"ab":
        for c in b"cd":
            pairs += 1
    print(pairs)  # 4
