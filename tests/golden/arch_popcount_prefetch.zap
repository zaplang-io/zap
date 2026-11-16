# Golden fixture: arch intrinsics `popcount_*` and `prefetch` (I#139 Tier 2,
# #962, D-0199). popcount lowers to the hardware population count (x86 `POPCNT` /
# aarch64 `CNT`) and is architecture-universal; its result is exact and the same
# everywhere. prefetch lowers to a locality hint (`PREFETCHh` / `PRFM`) that never
# faults and may be dropped — the program is correct whether or not it is honoured,
# which is exactly what this fixture asserts (it prefetches a buffer at every
# locality then uses it normally).
when target.arch == .x86_64:
    import arch.x86_64 as a
elif target.arch == .aarch64:
    import arch.aarch64 as a

def main():
    print(int64(a.popcount_u64(255)))  # 8 bits set
    print(int64(a.popcount_u32(4294967295)))  # 32 bits set
    print(int64(a.popcount_u64(0)))  # 0 bits set
    auto buf = b"abcdefgh"
    a.prefetch(buf)  # default locality (T0)
    a.prefetch(buf, 0)  # NTA (non-temporal)
    a.prefetch(buf, 3)  # T0 (all cache levels)
    print(len(buf))  # 8 — the hint changed nothing observable
