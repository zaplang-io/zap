# Golden (#1064): exact wyhash str-hash VALUES under a pinned seed
# (ZAPHASHSEED=1, set via the sibling .env). wyhash reads bytes little-endian on
# both supported targets, so these signed-int64 values are identical on x86_64
# and aarch64 — a cross-arch regression guard for the vendored algorithm. The
# strings span wyhash's length branches: empty, 1 byte (<4, wyr3 path), 3/4 bytes
# (<=16), 11/25 bytes (>16, <=48), and 50 bytes (>48, the 3-lane bulk loop).
# `str.__hash__()` lowers to the same seeded `zap_str_hash` the dict/set key path
# and `@derive(Hash)` use, so these ARE the hashes the dict sees this run.
# Companion hash_wyhash_seed2.zap prints "abc"/"hello world"/"" under seed 2 —
# different values — proving the per-process seed changes the hash.
def main():
    # Stable within a run: the same string hashes to the same value twice.
    print("abc".__hash__())  # 4978974695778740434
    print("abc".__hash__())  # 4978974695778740434

    print("".__hash__())  # -2473454724366372524
    print("x".__hash__())  # 5961981753439886648
    print("abcd".__hash__())  # -1572583294490056190
    print("hello world".__hash__())  # 1489843204584007197
    print("heap-grown-key-0123456789".__hash__())  # 1573231733126142189
    print("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMN".__hash__())  # -6426026575418430991
