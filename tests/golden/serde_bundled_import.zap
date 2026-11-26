# E6: the serde foundation is a BUNDLED pure-Zap stdlib module (Option B, a
# D-0081 extension) — resolved by `import serde` with no `zap add`, one blessed
# in-language version for the whole ecosystem. This golden proves the bundled
# module resolves, merges, and drives (de)serialization of a user's local types
# through a user's local backend that conforms to `serde.Serializer` /
# `serde.Deserializer` — all fully monomorphized across the module boundary
# (@...serde__serialize$Point$Sink in the lowered ZIR), with no compiler change
# per codec. The backend is the same trivial token stream used by the #1562 core
# proof; here it is driven entirely through the imported `serde` module.
import serde

struct Sink:
    list[str] toks = []

    def begin_struct(self):
        self.toks.append("<")

    def field_name(self, str name):
        pass

    def emit_int(self, int64 v):
        self.toks.append(str(v))

    def emit_float(self, float64 v):
        self.toks.append(str(v))

    def emit_str(self, str v):
        self.toks.append(v)

    def emit_bool(self, bool v):
        self.toks.append(str(v))

    def emit_bytes(self, bytes v):
        self.toks.append(str(v))

    def emit_uint(self, uint64 v):
        self.toks.append(str(v))

    def emit_nil(self):
        self.toks.append("nil")

    def begin_seq(self):
        self.toks.append("[")

    def end_seq(self):
        self.toks.append("]")

    def begin_map(self):
        self.toks.append("{")

    def end_map(self):
        self.toks.append("}")

    def end_struct(self):
        self.toks.append(">")

struct Src:
    list[str] toks = []
    int64 pos = 0

    def begin_struct(self):
        pass

    def read_int(self, str field) -> int64:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return int(t)

    def read_float(self, str field) -> float64:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return float(t)

    def read_str(self, str field) -> str:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return t

    def read_bool(self, str field) -> bool:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return t == "True"

    def read_bytes(self, str field) -> bytes:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return bytes(t)

    def read_uint(self, str field) -> uint64:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return uint64(int(t))

    def begin_optional(self, str field) -> bool:
        if self.toks[self.pos] == "nil":
            self.pos = self.pos + 1
            return False
        return True

    def begin_seq(self, str field) -> int64:
        return 0

    def end_seq(self):
        pass

    def begin_map(self, str field) -> int64:
        return 0

    def end_map(self):
        pass

    def end_struct(self):
        pass

struct Point:
    int64 x
    str label
    bool active

struct Outer:
    int64 id
    Point p

struct Measure:
    int64 id
    float64 score
    bytes tag

struct Coll:
    list[int64] nums
    dict[str, int64] tags
    uint64 big

struct Unsigned:
    uint64 n

struct Opt:
    int64 id
    Optional[int64] maybe
    Optional[str] note

def main():
    # Serialize a user struct through the imported serde driver + a local backend.
    auto p = Point(3, "hi", True)
    auto w = Sink()
    serde.serialize(p, w)
    print(",".join(w.toks) == "<,3,hi,True,>")

    # Deserialize it back (both type args explicit, as serde.deserialize requires).
    auto p2 = serde.deserialize[Point, Src](Src(["3", "hi", "True"], 0))
    print(p2.x == 3 and p2.label == "hi" and p2.active == True)

    # A nested struct field recurses into its own serde specialization.
    auto o = Outer(7, Point(1, "q", False))
    auto wo = Sink()
    serde.serialize(o, wo)
    print(",".join(wo.toks) == "<,7,<,1,q,False,>,>")

    # float64 and bytes fields through the bundled serde (breadth slice 1).
    auto m = Measure(1, 1.5, bytes("ok"))
    auto wm = Sink()
    serde.serialize(m, wm)
    print(",".join(wm.toks) == "<,1,1.5,ok,>")
    auto m2 = serde.deserialize[Measure, Src](Src(["1", "1.5", "ok"], 0))
    print(m2.id == 1 and m2.score == 1.5 and m2.tag == bytes("ok"))

    # list[int64] / dict[str,int64] / uint64 ENCODE through the bundled serde
    # (breadth slice 2). A single-entry dict keeps the token order deterministic.
    auto c = Coll([1, 2], {"k": 9}, uint64(7))
    auto wc = Sink()
    serde.serialize(c, wc)
    print(",".join(wc.toks) == "<,[,1,2,],{,k,9,},7,>")

    # uint64 scalar round-trips.
    auto u2 = serde.deserialize[Unsigned, Src](Src(["42"], 0))
    print(u2.n == uint64(42))

    # Optional[T] fields: Some emits the inner value, None emits a nil (breadth
    # slice 3). The decoder's begin_optional distinguishes them.
    auto opt = Opt(1, Optional.Some(9), Optional.None)
    auto wopt = Sink()
    serde.serialize(opt, wopt)
    print(",".join(wopt.toks) == "<,1,9,nil,>")
    auto opt2 = serde.deserialize[Opt, Src](Src(["1", "9", "nil"], 0))
    int64 got = -1
    match opt2.maybe:
        case Optional.Some(x):
            got = x
        case Optional.None:
            got = -1
    bool note_none = False
    match opt2.note:
        case Optional.Some(y):
            note_none = False
        case Optional.None:
            note_none = True
    print(opt2.id == 1 and got == 9 and note_none)
