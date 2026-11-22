# A sibling module exporting a struct with associated functions (#1260). Its
# public `Message` is reachable as `msg.Message`, and its receiverless `def`s
# (`query`, `from_wire`) are associated functions callable as
# `msg.Message.query(...)` / `msg.Message.from_wire(...)`.
struct Message:
    int64 id
    bytes payload

    # Named constructor (associated — no self).
    def query(int64 id) -> Message:
        return Message(id, b"")

    # Alternate constructor from a wire buffer: the first byte is the id.
    def from_wire(bytes data) -> Message:
        return Message(int64(data[0]), data)

    # Instance method (has self) — callable only on a value.
    def to_wire(self) -> bytes:
        return self.payload
