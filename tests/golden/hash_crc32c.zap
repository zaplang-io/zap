# Golden fixture: the portable `hash.crc32c` wrapper (I#139 Tier 2, #962, D-0199).
# A software CRC32C (Castagnoli) that runs on every target with no arch import,
# bit-identical to the hardware path. Checked against known vectors: the empty
# string is 0 (init 0xFFFFFFFF XOR final 0xFFFFFFFF), and "123456789" is the
# canonical CRC32C/iSCSI check value 0xE3069283 = 3792249987. Accepts str or
# bytes (same ZapStr layout). `crc32c` is Castagnoli — never the ISO/zlib CRC32.
def main():
    print(hash.crc32c("") == uint32(0))
    print(hash.crc32c("123456789") == uint32(3792249987))
    print(hash.crc32c(b"123456789") == uint32(3792249987))
