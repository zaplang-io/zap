# serde — the blessed, in-language serialization foundation for Zap.
#
# A format-agnostic (de)serialization layer over compile-time reflection (epic
# #1248): a `Serializer` / `Deserializer` backend pair plus the shared generic
# `serialize` / `deserialize` drivers. A codec (msgpack, cbor, avro, …) is a pure
# library — it implements the two backend interfaces and reuses these drivers,
# with NO per-codec compiler change. The field walk happens at COMPILE time via
# `comptime for` over `reflect.type_info(T).fields`, and the backend is an
# interface-bounded type parameter, so dispatch is fully monomorphized and
# unboxed — explicitly not runtime reflection.
#
# This module ships bundled with the compiler (Option B, D-0081 extension): it is
# resolved by `import serde` as a built-in pure-Zap stdlib module — one blessed,
# version-coherent implementation for the whole ecosystem, no `zap add`.
#
# Supported field kinds: `int64`, `float64`, `str`, `bool`, `bytes`, and a nested
# derived struct (encode recurses into the field's own specialization). Enum and
# generic-struct DESERIALIZE are blocked on the `reflect.construct` generalization
# (#1561) and are a later addition; collection / Optional breadth is a follow-up.

# A format backend emits a value as a sequence of visitor events. A concrete
# codec implements this once; `field_name` may be a no-op for positional formats.
interface Serializer:
    def begin_struct(self)
    def field_name(self, str name)
    def emit_int(self, int64 v)
    def emit_float(self, float64 v)
    def emit_str(self, str v)
    def emit_bool(self, bool v)
    def emit_bytes(self, bytes v)
    def end_struct(self)

# The decode mirror: a backend reads scalar values in field order.
interface Deserializer:
    def begin_struct(self)
    def read_int(self, str field) -> int64
    def read_float(self, str field) -> float64
    def read_str(self, str field) -> str
    def read_bool(self, str field) -> bool
    def read_bytes(self, str field) -> bytes
    def end_struct(self)

# Serialize `value` of any struct type `T` into the backend `s`. The backend is a
# `[S: Serializer]` type parameter, so each (T, S) pair is a distinct, fully
# monomorphized specialization. A nested struct field recurses with both type
# arguments inferred from the value arguments.
def serialize[T, S: Serializer](T value, S s):
    s.begin_struct()
    comptime for f in reflect.type_info(T).fields:
        s.field_name(comptime f.name)
        if f.ty.name == "int64":
            s.emit_int(reflect.field(value, comptime f.index))
        elif f.ty.name == "float64":
            s.emit_float(reflect.field(value, comptime f.index))
        elif f.ty.name == "str":
            s.emit_str(reflect.field(value, comptime f.index))
        elif f.ty.name == "bool":
            s.emit_bool(reflect.field(value, comptime f.index))
        elif f.ty.name == "bytes":
            s.emit_bytes(reflect.field(value, comptime f.index))
        elif f.ty.kind.name == "struct":
            serialize(reflect.field(value, comptime f.index), s)
        else:
            comptime.error(f.span, "serde.serialize: unsupported field kind")
    s.end_struct()

# Deserialize a value of struct type `T` from the backend `d`. Both type arguments
# are explicit at the call site (`serde.deserialize[T, D](d)`): `T` is the
# construct target and cannot be inferred. A codec library typically wraps this as
# `unpack[T](bytes) -> T`.
def deserialize[T, D: Deserializer](D d) -> T:
    auto out = reflect.construct(T)
    d.begin_struct()
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == "int64":
            reflect.set_field(out, comptime f.index, d.read_int(comptime f.name))
        elif f.ty.name == "float64":
            reflect.set_field(out, comptime f.index, d.read_float(comptime f.name))
        elif f.ty.name == "str":
            reflect.set_field(out, comptime f.index, d.read_str(comptime f.name))
        elif f.ty.name == "bool":
            reflect.set_field(out, comptime f.index, d.read_bool(comptime f.name))
        elif f.ty.name == "bytes":
            reflect.set_field(out, comptime f.index, d.read_bytes(comptime f.name))
        else:
            comptime.error(f.span, "serde.deserialize: unsupported field kind")
    d.end_struct()
    return out
