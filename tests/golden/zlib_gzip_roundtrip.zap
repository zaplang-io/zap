# Golden fixture: zlib/gzip compress->decompress round-trips (ticket #106).
#
# Both framings must round-trip losslessly: decompress(compress(x)) == x for the
# zlib wrapper (RFC 1950, Adler-32) and the gzip wrapper (RFC 1952, ISO CRC32).
# The decompressed bytes are printed as text (the input is ASCII) and by length;
# the COMPRESSED length is intentionally not asserted (it varies by zlib build).
# The default level and an explicit level both round-trip, and empty input is
# well-defined. `import zlib`/`import gzip` are accepted but not required.
def main():
    auto data = "Zap compression: the quick brown fox jumps over the lazy dog. The quick brown fox jumps over the lazy dog. The quick brown fox jumps over the lazy dog."
    # zlib framing, default level.
    auto zb = zlib.decompress(zlib.compress(data))
    print(zb)
    print(len(zb))
    # gzip framing, default level.
    auto gb = gzip.decompress(gzip.compress(data))
    print(gb)
    print(len(gb))
    # An explicit compression level round-trips too (9 = best, 1 = fastest).
    print(zlib.decompress(zlib.compress(data, 9)))
    print(gzip.decompress(gzip.compress(data, 1)))
    # Empty input is well-defined (the wrapper framing still round-trips to "").
    print(len(zlib.decompress(zlib.compress(""))))
    print(len(gzip.decompress(gzip.compress(""))))
