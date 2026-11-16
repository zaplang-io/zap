# Golden fixture: zlib/gzip keyword & default argument binding (#998).
#
# compress/decompress accept keyword arguments, in any order, with the optional
# `level` filled by name or omitted entirely — all lowering to the same runtime
# call as the positional form, so decompress(compress(x)) stays byte-identical.
# Reordered keywords (`level` before `data`) must still place each value in the
# right slot. gzip shares the migrated check/lower path. The compressed length
# is intentionally not asserted (it varies by zlib build).
def main():
    auto data = "the quick brown fox jumps over the lazy dog"
    # keyword data, default level (level omitted, filled by name)
    print(zlib.decompress(zlib.compress(data=data)))
    # keyword data + keyword level
    print(zlib.decompress(zlib.compress(data=data, level=9)))
    # reordered: level before data
    print(zlib.decompress(zlib.compress(level=1, data=data)))
    # gzip shares the path; keyword form
    print(gzip.decompress(gzip.compress(data=data, level=6)))
    # decompress bound by keyword too
    print(gzip.decompress(data=gzip.compress(data)))
