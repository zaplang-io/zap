# Associated functions on struct/enum types (#1260): a receiverless `def` (its
# first parameter is not `self`) is an associated function, callable only as
# `Type.func(...)`. This exercises a named constructor, an alternate constructor
# from bytes, keyword arguments, a non-self return type, coexistence with an
# instance method and the field constructor, and an enum associated function
# alongside D-0046 tier-3 variant construction.

struct Message:
    int64 id
    bytes payload

    # Named constructor (associated — no self).
    def query(int64 id) -> Message:
        return Message(id, b"")

    # Alternate constructor from a wire buffer: the first byte is the id.
    def from_wire(bytes data) -> Message:
        return Message(int64(data[0]), data)

    # Multi-arg associated constructor.
    def tagged(int64 id, bytes payload) -> Message:
        return Message(id, payload)

    # An associated function returning a non-self type.
    def kind() -> str:
        return "message"

    # Instance method (has self) — stays callable only on a value.
    def to_wire(self) -> bytes:
        return self.payload

enum Signal:
    LOW
    HIGH

    # Associated function on an enum, returning the enum.
    def on() -> Signal:
        return Signal.HIGH

enum Shape:
    Circle(f64 r)
    Square(f64 s)

    # Associated (named) constructor returning a tier-3 variant.
    def unit_circle() -> Shape:
        return Shape.Circle(1.0)

def main():
    # Named constructor returning the declaring type.
    auto a = Message.query(42)
    print(a.id)  # 42

    # Keyword argument at the call site.
    auto b = Message.query(id=7)
    print(b.id)  # 7

    # Alternate constructor from bytes; instance method on the result.
    auto c = Message.from_wire(bytes([5, 104, 105]))
    print(c.id)  # 5
    print(len(c.to_wire()))  # 3

    # Multi-arg associated constructor, mixed positional + keyword.
    auto d = Message.tagged(9, payload=b"abcd")
    print(d.id)  # 9
    print(len(d.to_wire()))  # 4

    # Associated function with a non-self return type.
    print(Message.kind())  # message

    # The positional field constructor still works alongside the named ones.
    auto e = Message(1, b"xy")
    print(e.id)  # 1
    print(len(e.to_wire()))  # 2

    # Enum associated function + a tier-1 variant value.
    auto s = Signal.on()
    print(s == Signal.HIGH)  # True

    # Enum named constructor returning a tier-3 variant; variant construction
    # (`Shape.Circle(...)`) still resolves alongside it.
    auto u = Shape.unit_circle()
    match u:
        case Shape.Circle(r):
            print(r)  # 1
        case Shape.Square(s):
            print(s)
