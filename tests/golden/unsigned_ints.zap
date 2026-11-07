# Golden fixture: unsigned integer types (#552, D-0019 amendment).
#
# Reintroduced for C FFI (#539) and array dtypes. This fixture pins the runtime
# semantics that only the full LLVM+codegen CI job can observe (local LLVM is
# unavailable): unsigned printing (no phantom sign; uint64 above INT64_MAX shows
# its true magnitude), two's-complement wraparound on narrowing, unsigned
# division/modulo/comparison/shift that differ from the signed ops, and the
# explicit cross-sign conversions. Every width maximum is built with `uintN(-1)`
# (a C reinterpret cast) so no 64-bit-unsigned decimal literal is needed.
#
# Arithmetic promotes to uint64 and wraps modulo 2^64 (mirroring the signed
# tower's promotion to int64, D-0019); a narrower unsigned result comes from an
# explicit `uintN(...)` cast. `usize` is the pointer-width unsigned type.

def udiv(uint64 a, uint64 b) -> uint64:
    return a // b

def main():
    # The maximum of each width prints as an unsigned decimal.
    print(uint8(-1))  # 255
    print(uint16(-1))  # 65535
    print(uint32(-1))  # 4294967295
    print(uint64(-1))  # 18446744073709551615

    # Small-value arithmetic promotes to uint64; a narrowing cast wraps mod 256.
    uint8 a = 200
    uint8 b = 100
    print(a + b)  # 300  (computed at uint64 width, no 8-bit wrap yet)
    print(uint8(a + b))  # 44   (explicit narrowing wraps modulo 256)

    # Unsigned comparison: 2^64-1 > 1 (a signed read would see the bits as -1).
    uint64 big = uint64(-1)
    uint64 one = 1
    print(big > one)  # True
    print(big < one)  # False

    # Unsigned division / modulo (signed -1 // 2 would be 0, -1 % 2 would be -1).
    print(big // 2)  # 9223372036854775807
    print(big % 2)  # 1
    print(udiv(big, 2))  # 9223372036854775807  (through a typed uint64 fn)

    # Logical right shift zero-fills (a signed -1 >> 60 would stay all-ones).
    print(big >> 60)  # 15  (constant count: inline lshr)
    uint64 sh = 60
    print(big >> sh)  # 15  (variable count: guarded zap_ushr)

    # Power wraps modulo 2^64.
    uint64 base = 2
    uint64 exp = 10
    print(base ** exp)  # 1024

    # Explicit reinterpret back to signed: uint64(-1) is -1 as an int64.
    print(int64(big))  # -1

    # usize is pointer-width unsigned and uses the same unsigned ops.
    usize n = 100
    print(n)  # 100
    print(n // usize(7))  # 14

    # uint8 is still the dense array dtype (image data, #49): a[i] zero-extends.
    array[uint8] buf = array[uint8]([250, 251, 252])
    print(buf[0])  # 250
