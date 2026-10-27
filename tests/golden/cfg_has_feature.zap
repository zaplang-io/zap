# Compile-time feature probe (I#139 / D-0079). `target.has(.f)` is true only
# when `f` was baked into the build via `--target-feature`. The golden harness
# passes no feature flag, so the `else` arm is selected and the feature arm
# (which names an undefined intrinsic) is skipped as a dead arm.
when target.has(.sse4_2):
    def checksum() -> int64:
        return hardware_crc32c()
else:
    def checksum() -> int64:
        return 0

def main():
    print(checksum())
