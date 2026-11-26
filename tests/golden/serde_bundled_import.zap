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

    def emit_str(self, str v):
        self.toks.append(v)

    def emit_bool(self, bool v):
        self.toks.append(str(v))

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

    def read_str(self, str field) -> str:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return t

    def read_bool(self, str field) -> bool:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return t == "True"

    def end_struct(self):
        pass

struct Point:
    int64 x
    str label
    bool active

struct Outer:
    int64 id
    Point p

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
