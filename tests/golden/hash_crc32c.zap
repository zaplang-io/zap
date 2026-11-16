# Golden fixture: the portable `hash.crc32c` wrapper (I#139 Tier 2, #962, D-0199).
# A software CRC32C (Castagnoli) that runs on every target with no arch import,
# bit-identical to the hardware path. Checked against known vectors: the empty
# string is 0 (init 0xFFFFFFFF XOR final 0xFFFFFFFF), and "123456789" is the
# canonical CRC32C/iSCSI check value 0xE3069283 = 3808858755. Accepts str or
# bytes (same ZapStr layout). `crc32c` is Castagnoli — never the ISO/zlib CRC32.
#
# The 32-bit checksum is widened to int64 and masked to its low 32 bits
# (`& 0xFFFFFFFF`) before comparison against the plain int64 check value.
def crc(bytes data) -> int64:
    return int64(hash.crc32c(data)) & 4294967295

def crcs(str data) -> int64:
    return int64(hash.crc32c(data)) & 4294967295

def main():
    print(crcs("") == 0)
    print(crcs("123456789") == 3808858755)
    print(crc(b"123456789") == 3808858755)
