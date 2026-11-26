# E6.2 (#1563) — a real binary format backend on the serde core.
#
# Validates the serde visitor model (#1562) end-to-end against REAL BYTES: a
# compact self-describing TLV binary codec implemented purely as a `Serializer` /
# `Deserializer` backend, driven by the same generic `serialize` / `deserialize`
# free functions — NO compiler change, NO per-codec special-casing. The serde core
# is re-declared here (it ships as a self-contained proof, not yet an importable
# module); E6.3 (the msgpack repo) is where the core must become importable.
#
# Wire format (deterministic, byte-checkable):
#   begin_struct = 0x10, end_struct = 0x11
#   int64  = 0x01 + 8 bytes big-endian (two's complement)
#   str    = 0x02 + 1-byte length + UTF-8 bytes
#   bool   = 0x03 + 1 byte (0/1)
# Field names are not written (positional; the decoder walks reflect fields in
# declaration order).

interface Serializer:
    def begin_struct(self)
    def field_name(self, str name)
    def emit_int(self, int64 v)
    def emit_str(self, str v)
    def emit_bool(self, bool v)
    def end_struct(self)

interface Deserializer:
    def begin_struct(self)
    def read_int(self, str field) -> int64
    def read_str(self, str field) -> str
    def read_bool(self, str field) -> bool
    def end_struct(self)

def serialize[T, S: Serializer](T value, S s):
    s.begin_struct()
    comptime for f in reflect.type_info(T).fields:
        s.field_name(comptime f.name)
        if f.ty.name == "int64":
            s.emit_int(reflect.field(value, comptime f.index))
        elif f.ty.name == "str":
            s.emit_str(reflect.field(value, comptime f.index))
        elif f.ty.name == "bool":
            s.emit_bool(reflect.field(value, comptime f.index))
        elif f.ty.kind.name == "struct":
            serialize(reflect.field(value, comptime f.index), s)
        else:
            comptime.error(f.span, "serialize: unsupported field kind")
    s.end_struct()

def deserialize[T, D: Deserializer](D d) -> T:
    auto out = reflect.construct(T)
    d.begin_struct()
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == "int64":
            reflect.set_field(out, comptime f.index, d.read_int(comptime f.name))
        elif f.ty.name == "str":
            reflect.set_field(out, comptime f.index, d.read_str(comptime f.name))
        elif f.ty.name == "bool":
            reflect.set_field(out, comptime f.index, d.read_bool(comptime f.name))
        else:
            comptime.error(f.span, "deserialize: unsupported field kind")
    d.end_struct()
    return out

# --- the binary backend -------------------------------------------------------
struct BinWriter:
    list[int64] buf = []

    def begin_struct(self):
        self.buf.append(16)

    def field_name(self, str name):
        pass

    def emit_int(self, int64 v):
        self.buf.append(1)
        for k in range(8):
            auto shift = (7 - k) * 8
            self.buf.append(v >> shift & 255)

    def emit_str(self, str v):
        self.buf.append(2)
        auto sb = bytes(v)
        self.buf.append(len(sb))
        for i in range(len(sb)):
            self.buf.append(sb[i])

    def emit_bool(self, bool v):
        self.buf.append(3)
        if v:
            self.buf.append(1)
        else:
            self.buf.append(0)

    def end_struct(self):
        self.buf.append(17)

    def done(self) -> bytes:
        return bytes(self.buf)

struct BinReader:
    bytes data = bytes([])
    int64 pos = 0

    def _byte(self) -> int64:
        auto b = self.data[self.pos]
        self.pos = self.pos + 1
        return b

    def begin_struct(self):
        auto _t = self._byte()

    def read_int(self, str field) -> int64:
        auto _t = self._byte()
        int64 v = 0
        for k in range(8):
            v = v << 8 | self._byte()
        return v

    def read_str(self, str field) -> str:
        auto _t = self._byte()
        auto n = self._byte()
        list[int64] raw = []
        for i in range(n):
            raw.append(self._byte())
        return str(bytes(raw))

    def read_bool(self, str field) -> bool:
        auto _t = self._byte()
        auto b = self._byte()
        return b == 1

    def end_struct(self):
        auto _t = self._byte()

# --- types under test ---------------------------------------------------------
struct Point:
    int64 x
    str label
    bool active

struct Edge:
    int64 n
    str s
    bool b

struct Msg:
    str text

struct Outer:
    int64 id
    Point p

def main():
    # 1. Flat struct serializes to the exact wire bytes.
    auto p = Point(3, "hi", True)
    auto w = BinWriter()
    serialize(p, w)
    auto wire = w.done()
    print(wire == bytes([16, 1, 0, 0, 0, 0, 0, 0, 0, 3, 2, 2, 104, 105, 3, 1, 17]))

    # 2. ... and round-trips back through the same backend.
    auto p2 = deserialize[Point, BinReader](BinReader(wire, 0))
    print(p2.x == 3 and p2.label == "hi" and p2.active == True)

    # 3. Edge scalars (negative int two's-complement, empty string, False).
    auto e = Edge(-5, "", False)
    auto we = BinWriter()
    serialize(e, we)
    auto e2 = deserialize[Edge, BinReader](BinReader(we.done(), 0))
    print(e2.n == -5 and e2.s == "" and e2.b == False)

    # 4. A multi-byte UTF-8 string round-trips (length is in bytes, not codepoints).
    auto m = Msg("héllo")
    auto wm = BinWriter()
    serialize(m, wm)
    auto m2 = deserialize[Msg, BinReader](BinReader(wm.done(), 0))
    print(m2.text == "héllo")

    # 5. A nested struct field ENCODES as nested frames (byte-exact).
    auto o = Outer(7, Point(1, "q", False))
    auto wo = BinWriter()
    serialize(o, wo)
    print(
        wo.done() == bytes([16, 1, 0, 0, 0, 0, 0, 0, 0, 7, 16, 1, 0, 0, 0, 0, 0, 0, 0, 1, 2, 1, 113, 3, 0, 17, 17]),
    )
