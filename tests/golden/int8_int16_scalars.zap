# Golden fixture: int8 / int16 as first-class signed narrow scalar value types
# (#1654, decisions/lang/1654-fixed-width-integer-semantics.md).
#
# int8/int16 are the signed mirrors of uint8/uint16. Fixed-width integer
# arithmetic is width-stable and wraps at the operand width (two's-complement),
# so a same-width op stays that width and wraps, a mixed-width op widens to the
# larger operand, and an untyped literal adopts the concrete operand's width.
# This pins the RUNTIME behaviour the full LLVM+codegen CI job observes (local
# LLVM is unavailable); the arithmetic-wrap narrowing itself is I#1677, which
# already covers I8/I16.

def main():
    # Same-width int8 add overflows past 127 and wraps (100 + 50 = 150 -> -106).
    int8 a = 100
    int8 b = 50
    print(a + b)  # -106   (150 - 256)

    # Subtraction underflows past -128 and wraps (-100 - 100 = -200 -> 56).
    int8 c = -100
    int8 d = 100
    print(c - d)  # 56     (-200 + 256)

    # An untyped literal adopts the int8 operand width, then wraps (100 * 2).
    int8 e = 100
    print(e * 2)  # -56    (200 - 256)

    # Two negative int8 operands wrap on underflow (-100 + -50 = -150 -> 106).
    int8 f = -100
    int8 g = -50
    print(f + g)  # 106    (-150 + 256)

    # A literal in int8 arithmetic adopts the width and wraps (120 + 10 = 130).
    int8 base = 120
    print(base + 10)  # -126   (130 - 256)

    # Explicit int8() is the wrap escape hatch (C-cast semantics).
    print(int8(300))  # 44     (300 mod 256)
    print(int8(-130))  # 126    (-130 + 256)

    # The counter idiom keeps the int8 width and wraps on overflow.
    int8 cnt = 127
    cnt += 1
    print(cnt)  # -128   (128 - 256)

    # int16 same-width add overflows past 32767 and wraps.
    int16 p = 30000
    int16 q = 30000
    print(p + q)  # -5536  (60000 - 65536)

    # Two negative int16 operands wrap on underflow.
    int16 r = -30000
    int16 s = -30000
    print(r + s)  # 5536   (-60000 + 65536)

    # Explicit int16() wrap.
    print(int16(70000))  # 4464   (70000 mod 65536)

    # Mixed int8 + int16 widens to int16; 30100 fits, so no wrap.
    int8 m8 = 100
    int16 m16 = 30000
    print(m8 + m16)  # 30100

    # Implicit widening up the signed tower sign-extends (no wrap).
    int8 w = -5
    int32 wc = w
    print(wc)  # -5
    int16 ww = -5
    int64 wd = ww
    print(wd)  # -5

    # Comparison yields bool, at the narrow width.
    int8 ca = int8(5)
    int8 cb = int8(10)
    print(ca < cb)  # True
