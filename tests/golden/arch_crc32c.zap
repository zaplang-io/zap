# DIAGNOSTIC (temporary) — print the actual CRC32C values CI computes.
when target.arch == .x86_64:
    import arch.x86_64 as a
elif target.arch == .aarch64:
    import arch.aarch64 as a

def fold_crc32c(bytes data) -> int64:
    int64 crc = 4294967295
    for i in range(0, len(data)):
        crc = int64(a.crc32c_u8(crc, data[i]))
    return (crc ^ 4294967295) & 4294967295

def main():
    auto data = b"123456789"
    print(fold_crc32c(data))
    print(int64(hash.crc32c(data)) & 4294967295)
    print(int64(a.crc32c_u8(4294967295, 49)) & 4294967295)
    print(len(data))
    print(int64(data[0]))
