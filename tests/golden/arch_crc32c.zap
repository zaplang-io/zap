# Golden fixture: the curated arch intrinsics module (I#139 Tier 2, #962, D-0199)
# — hardware CRC32C (Castagnoli), end to end.
#
# `when target.arch` keeps exactly the one arch module that matches the build
# (x86_64 or aarch64 on CI); both lower `crc32c_u8` to that arch's hardware CRC32C
# instruction (x86 SSE4.2 `crc32`, aarch64 CRC32C ext). Folding the hardware step
# over the bytes with the standard CRC32C init (0xFFFFFFFF) / final XOR
# (0xFFFFFFFF) must (a) equal the portable `hash.crc32c` wrapper and (b) equal the
# well-known CRC32C check value of "123456789", 0xE3069283 = 3792249987. Because
# the result is value-identical on every arch and CPU, this golden's output is the
# same on every CI box. CRC32C is Castagnoli, NOT the ISO/zlib CRC32.
when target.arch == .x86_64:
    import arch.x86_64 as a
elif target.arch == .aarch64:
    import arch.aarch64 as a

def fold_crc32c(bytes data) -> uint32:
    uint32 crc = 4294967295
    for i in range(0, len(data)):
        crc = a.crc32c_u8(crc, data[i])
    return crc ^ 4294967295

def main():
    auto data = b"123456789"
    auto raw = fold_crc32c(data)
    auto lib = hash.crc32c(data)
    # The friendly wrapper is identical to the raw hardware intrinsic folded.
    print(raw == lib)
    # ...and both equal the canonical CRC32C/iSCSI check value (0xE3069283).
    print(raw == uint32(3792249987))
