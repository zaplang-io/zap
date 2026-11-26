# E6.1 (#1562) — serde CORE round-trip over comptime reflection.
#
# This is the payoff of epic #1248: a *general* serialization layer where a codec
# is a pure library — a `Serializer` / `Deserializer` backend plus the shared
# generic `serialize` / `deserialize` functions — with NO per-codec compiler
# change. The field walk is done at COMPILE time via `comptime for` over
# `reflect.type_info(T).fields`; the backend dispatch is fully MONOMORPHIZED and
# UNBOXED (a `[S: Serializer]` bound resolves statically per concrete backend — no
# fat pointer / vtable; cf. `generics_bounds.zap`), so this is explicitly NOT
# Go/Java runtime reflection.
#
# The crux (an E6 design finding): a monomorphized backend CANNOT be threaded
# through a `@derive`'d method — interface methods cannot carry their own generics,
# and the `@derive` forwarding stub is non-generic (crates/driver/src/derive_synth.rs).
# So the core is expressed as FREE GENERIC FUNCTIONS, the idiomatic Zap form (cf.
# the msgpack library's `packb_with_default[T]`). The lowered ZIR shows direct
# `fieldget`/`fieldset` and direct `@Backend__emit_*` calls, nested structs
# specialized inline.
#
# Scope of this slice: scalar fields (int64 / str / bool) and nested struct ENCODE.
# Enum / generic-struct DESERIALIZE is blocked on the `reflect.construct`
# generalization (#1561, filed under #1248) and is NOT attempted here.

# --- serde core: the format-agnostic backend interfaces ----------------------
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

# --- serde core: the shared generic drivers (one impl, every backend) --------
# A nested struct field recurses via `serialize(...)` with both type arguments
# inferred from the value arguments — the nested backend specialization is
# produced automatically.
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

# `deserialize` zero-constructs a `T` then writes each field from the backend.
# Both type arguments are explicit at the call site (`deserialize[T, D](d)`): `T`
# cannot be inferred — it is the construct target — and a partially-explicit type
# argument list does not resolve. A codec library hides this behind `unpack[T]`.
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

# --- a trivial backend pair: a flat token stream (proves the visitor model) ---
# Structural tokens are `<` (begin) and `>` (end); every scalar is one token.
struct TokenSink:
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

# Reads scalar tokens positionally in field order (structural frames stripped by
# the caller). Mirrors the backend above, closing the round trip.
struct TokenSource:
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

# Strip the structural frames, leaving only scalar value tokens in field order.
def scalars_of(TokenSink sink) -> list[str]:
    list[str] out = []
    for t in sink.toks:
        if t != ">" and not t.startswith("<"):
            out.append(t)
    return out

# --- types under test --------------------------------------------------------
struct Point:
    int64 x
    int64 y
    str label
    bool active

struct Edge:
    int64 n
    str s
    bool b

struct Single:
    int64 only

struct Outer:
    int64 id
    Point p

def main():
    # 1. Flat struct round-trips every field through the generic core.
    auto p = Point(3, 4, "origin", True)
    auto psink = TokenSink()
    serialize(p, psink)
    auto p2 = deserialize[Point, TokenSource](TokenSource(scalars_of(psink), 0))
    print(p2.x == 3 and p2.y == 4 and p2.label == "origin" and p2.active == True)

    # 2. The encoded flat token stream is exactly begin + fields + end.
    print(",".join(psink.toks) == "<,3,4,origin,True,>")

    # 3. Edge scalar values (negative int, empty string, False) round-trip.
    auto e = Edge(-5, "", False)
    auto esink = TokenSink()
    serialize(e, esink)
    auto e2 = deserialize[Edge, TokenSource](TokenSource(scalars_of(esink), 0))
    print(e2.n == -5 and e2.s == "" and e2.b == False)

    # 4. A single-field struct round-trips (smallest non-empty case).
    auto one = Single(99)
    auto osink = TokenSink()
    serialize(one, osink)
    auto one2 = deserialize[Single, TokenSource](TokenSource(scalars_of(osink), 0))
    print(one2.only == 99)

    # 5. A nested struct field ENCODES by recursing into its own specialization.
    auto outer = Outer(7, Point(1, 2, "q", False))
    auto nsink = TokenSink()
    serialize(outer, nsink)
    print(",".join(nsink.toks) == "<,7,<,1,2,q,False,>,>")
