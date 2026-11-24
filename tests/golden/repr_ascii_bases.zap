# Golden fixture: repr / ascii / bin / oct / hex built-ins (#1384).
#
# repr(x) is the debug form (the RenderMode::Repr side of the Display protocol):
# strings quoted, bytes as b'...', containers rendered with element repr, scalars
# as their value. ascii(x) is repr with every non-ASCII codepoint backslash-
# escaped (\xNN <= 0xff, \uNNNN <= 0xffff, else \UNNNNNNNN). bin/oct/hex render an
# integer in the Python sign-magnitude base form with the 0b/0o/0x prefix.

def main():
    # --- repr of built-in scalars ---
    print(repr("hi"))  # 'hi'
    print(repr("it's"))  # "it's"  (has ', no " -> double-quoted)
    print(repr("a\nb"))  # 'a\nb'  (newline escaped)
    print(repr(42))  # 42
    print(repr(True))  # True
    print(repr(False))  # False
    print(repr(1.5))  # 1.5
    print(repr(chr(65)))  # 'A'  (a char is quoted in repr)
    print(repr(bytes("hi")))  # b'hi'

    # --- repr of containers: elements render in repr (quoted) ---
    print(repr([1, 2, 3]))  # [1, 2, 3]
    print(repr(["a", "b"]))  # ['a', 'b']
    print(repr([1.5, 2.5]))  # [1.5, 2.5]
    print(repr((1, "x")))  # (1, 'x')
    print(repr([]))  # []

    # --- ascii: non-ASCII escaped, ASCII passthrough ---
    print(ascii("abc"))  # 'abc'
    print(ascii("café"))  # 'caf\xe9'  (U+00E9 -> \xNN)
    print(ascii("日本"))  # '日本'  (BMP -> \uNNNN)
    print(ascii("😀"))  # '\U0001f600'  (astral -> \UNNNNNNNN)
    print(ascii(42))  # 42

    # --- bin / oct / hex: positive, zero, negative ---
    print(bin(10))  # 0b1010
    print(oct(10))  # 0o12
    print(hex(255))  # 0xff
    print(bin(0))  # 0b0
    print(oct(0))  # 0o0
    print(hex(0))  # 0x0
    print(bin(-5))  # -0b101
    print(oct(-9))  # -0o11
    print(hex(-255))  # -0xff

    # --- unsigned: true magnitude, never a phantom sign ---
    print(hex(uint64(255)))  # 0xff
    print(hex(uint64(-1)))  # 0xffffffffffffffff  (u64::MAX, not -0x1)

    # --- int64 min edge (unspellable as a literal: build by subtraction) ---
    int64 lo = -9223372036854775807 - 1
    print(hex(lo))  # -0x8000000000000000

    # --- bigint (limb short-division, incl. multi-limb) ---
    print(hex(bigint(255)))  # 0xff
    print(hex(bigint(-255)))  # -0xff
    print(bin(bigint(10)))  # 0b1010
    print(oct(bigint(8)))  # 0o10
    print(hex(bigint(4294967296)))  # 0x100000000  (2^32: two limbs)
