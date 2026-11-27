# Golden fixture: width-stable fixed-width integer arithmetic (#1654,
# decisions/lang/1654-fixed-width-integer-semantics.md; codegen half, I#1677).
#
# Fixed-width integer arithmetic is width-stable and wraps at the operand width
# (two's-complement), superseding the promote-to-64 clause of D-0019 / #552.
# This pins the RUNTIME wrap that only the full LLVM+codegen CI job can observe
# (local LLVM is unavailable): a same-width op stays that width and wraps, a
# mixed-width op widens to the larger operand, an untyped literal adopts the
# concrete operand's width, and `int64`/`uint64` are unaffected.

def main():
    # uint8 wraps modulo 256.
    uint8 u8a = 200
    uint8 u8b = 100
    print(u8a + u8b)  # 44     (300 mod 256)
    uint8 u8c = 50
    print(u8c - u8b)  # 206    (50 - 100 = -50, mod 256)
    print(u8a * u8b)  # 32     (20000 mod 256)

    # uint16 wraps modulo 65536.
    uint16 u16a = 60000
    uint16 u16b = 10000
    print(u16a + u16b)  # 4464   (70000 mod 65536)

    # uint32 wraps modulo 2^32.
    uint32 u32a = 4000000000
    uint32 u32b = 1000000000
    print(u32a + u32b)  # 705032704   (5000000000 mod 2^32)

    # int32 wraps modulo 2^32 (two's-complement signed read).
    int32 i32a = 2000000000
    int32 i32b = 2000000000
    print(i32a + i32b)  # -294967296  (4000000000 wrapped as int32)

    # An untyped literal adopts the concrete operand's width, then wraps.
    uint8 base = 250
    print(base + 10)  # 4      (260 mod 256)

    # Mixed width widens to the larger operand (no wrap when it fits).
    uint8 small = 200
    uint16 big = 500
    print(small + big)  # 700

    # int64 is unaffected at these magnitudes (no wrap).
    int64 w = 2000000000
    print(w + w)  # 4000000000

    # Power wraps at the narrow width.
    uint8 pbase = 2
    uint8 pexp = 10
    print(pbase ** pexp)  # 0      (1024 mod 256)

    # The counter idiom keeps the uint8 width and wraps on overflow.
    uint8 cnt = 255
    cnt += 1
    print(cnt)  # 0      (256 mod 256)
