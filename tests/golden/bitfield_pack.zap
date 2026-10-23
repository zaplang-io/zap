# Golden fixture: C-style bitfields on a value struct (I#173, D-0068).
#
# `Pack` declares four bitfields that pack into one 16-bit backing word:
#   int64 a : 4   (signed 4-bit, -8..7)   bits 0..3
#   int64 b : 4   (signed 4-bit, -8..7)   bits 4..7
#   Color  c : 2  (enum, 0..3)            bits 8..9
#   bool   d : 1                          bit  10        (11 bits total -> u16)
#
# The program proves: each field round-trips (including the signed max 7 and
# min -8, the enum's top value, and a true bool); writing one field leaves its
# neighbours untouched (adjacent-field independence, no corruption); and the
# packed value struct works through a method, inside a `list`, and as a generic
# type argument. Deterministic stdout, green under both collectors.

enum Color:
    RED  # 0
    GREEN  # 1
    BLUE  # 2
    WHITE  # 3  -> needs 2 bits

value struct Pack:
    int64 a: 4
    int64 b: 4
    Color c: 2
    bool d: 1

    # A method on a packed value struct: reads two bitfields and adds them.
    def ab_sum(self) -> int64:
        return self.a + self.b

# bool -> 0/1 so stdout is deterministic.
def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

# Generic identity: exercises the packed value struct as a generic type arg.
def echo[T](T x) -> T:
    return x

def main():
    # Set each field to a boundary value and read it back.
    auto p = Pack(a=7, b=-8, c=Color.WHITE, d=True)
    print(p.a)  # 7   (signed 4-bit max)
    print(p.b)  # -8  (signed 4-bit min, sign-extended on read)
    print(b2i(p.c == Color.WHITE))  # 1 (enum top value round-trips)
    print(b2i(p.d))  # 1
    print(p.ab_sum())  # -1  (7 + -8, via a method)

    # Adjacent-field independence: mutate only `b`; a, c, d must be unchanged.
    p.b = 5
    print(p.a)  # 7   — untouched
    print(p.b)  # 5
    print(b2i(p.c == Color.WHITE))  # 1 — untouched
    print(b2i(p.d))  # 1   — untouched

    # Flip `d` off; neighbours still intact.
    p.d = False
    print(b2i(p.d))  # 0
    print(p.a)  # 7
    print(p.b)  # 5

    # Value semantics (D-0051): a copy's mutation does not escape.
    auto q = p
    q.a = -1
    print(p.a)  # 7   — original unchanged
    print(q.a)  # -1

    # Inside a collection (boxed copy) — fields read back correctly.
    auto xs = [Pack(a=1, b=2, c=Color.GREEN, d=True)]
    print(xs[0].a)  # 1
    print(xs[0].b)  # 2
    print(b2i(xs[0].c == Color.GREEN))  # 1

    # As a generic type argument.
    auto r = echo(Pack(a=3, b=-4, c=Color.BLUE, d=False))
    print(r.a)  # 3
    print(r.b)  # -4
    print(b2i(r.c == Color.BLUE))  # 1
    print(b2i(r.d))  # 0
