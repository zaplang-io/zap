# Regression #537: LOS mappings must include the aligned metadata reservation,
# not just sizeof(ZapBlock). Payloads near a 32 KiB multiple used to overflow the
# allocation by up to one metadata-alignment gap. Exercise both sides of those
# boundaries, keep the bytes live through collection, then release them.
def main():
    auto seed = bytes([0, 255, 13, 10])
    for i in range(0, 14):
        seed = seed + seed
    list[bytes] keep = []
    list[int64] sizes = []
    for boundary in [32768, 65536]:
        for size in range(boundary - 432, boundary - 287):
            auto payload = seed[0:size]
            if len(payload) != size:
                raise ValueError("incorrect payload size")
            keep.append(payload)
            sizes.append(size)
    gc.collect()
    auto i = 0
    while i < len(keep):
        bytes payload = keep[i]
        auto size = sizes[i]
        if len(payload) != size:
            raise ValueError("payload size changed after collection")
        if payload != seed[0:size]:
            raise ValueError("payload changed after collection")
        i = i + 1
    keep = []
    gc.collect()
    print("LOS boundary allocations: PASS")
