# Golden fixture: zlib.crc32 / zlib.adler32 known vectors (ticket #106).
#
# The CRC32 is the ISO/gzip CRC32 (polynomial 0x04C11DB7), NOT CRC32C. Vectors:
#   crc32("123456789")  = 0xCBF43926 = 3421780262  (the standard CRC check value)
#   crc32(fox sentence) = 0x414FA339 = 1095738169
#   crc32("")           = 0
#   adler32("Wikipedia")= 0x11E60398 = 300286872    (the canonical Adler-32 example)
#   adler32("123456789")= 152961502
#   adler32("")         = 1
# Both are returned as unsigned 32-bit values widened to int.
def main():
    print(zlib.crc32("123456789"))
    print(zlib.crc32("The quick brown fox jumps over the lazy dog"))
    print(zlib.crc32(""))
    print(zlib.adler32("Wikipedia"))
    print(zlib.adler32("123456789"))
    print(zlib.adler32(""))
