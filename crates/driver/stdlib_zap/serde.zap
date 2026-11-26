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
# Supported field kinds: `int64`, `uint64`, `float64`, `str`, `bool`, `bytes`, a
# nested derived struct, `list[T]` / `dict[str, V]` of scalar elements, `Optional[T]`
# of a scalar (all both directions), and `enum` types (encode: all variants;
# decode: payload-free/constant variants, via reflect.construct_variant). Generic
# structs encode/decode via reflect.construct; payload-carrying enum DECODE is a
# later addition.

# A format backend emits a value as a sequence of visitor events. A concrete
# codec implements this once; `field_name` may be a no-op for positional formats.
interface Serializer:
    def begin_struct(self)
    def field_name(self, str name)
    def emit_int(self, int64 v)
    def emit_uint(self, uint64 v)
    def emit_float(self, float64 v)
    def emit_str(self, str v)
    def emit_bool(self, bool v)
    def emit_bytes(self, bytes v)
    # `Optional` None; a `Some` emits the inner value directly.
    def emit_nil(self)
    # An enum variant's tag (its name); any payload values follow via `emit_*`.
    def emit_variant(self, str name)
    # Sequence (list) and map (dict) framing. The driver calls `begin_*`, emits
    # the elements / key+value pairs, then `end_*`. A length-prefixed backend
    # buffers and backfills the header at `end_*` (no count up front on encode).
    def begin_seq(self)
    def end_seq(self)
    def begin_map(self)
    def end_map(self)
    def end_struct(self)

# The decode mirror: a backend reads scalar values in field order. For a sequence
# or map, `begin_seq` / `begin_map` return the element / entry count, then the
# driver reads that many elements / key+value pairs, then `end_seq` / `end_map`.
interface Deserializer:
    def begin_struct(self)
    def read_int(self, str field) -> int64
    def read_uint(self, str field) -> uint64
    def read_float(self, str field) -> float64
    def read_str(self, str field) -> str
    def read_bool(self, str field) -> bool
    def read_bytes(self, str field) -> bytes
    # `Optional`: return True and leave the value to be read (a `Some`), or consume
    # a nil and return False (a `None`).
    def begin_optional(self, str field) -> bool
    # An enum variant's tag (its name), read before any payload values.
    def read_variant(self) -> str
    def begin_seq(self, str field) -> int64
    def end_seq(self)
    def begin_map(self, str field) -> int64
    def end_map(self)
    def end_struct(self)

# Serialize `value` of any struct or enum type `T` into the backend `s`. The
# backend is a `[S: Serializer]` type parameter, so each (T, S) pair is a distinct,
# fully monomorphized specialization. The top-level kind dispatch is pruned per
# instantiation (a struct never compiles the enum arm, and vice versa).
def serialize[T, S: Serializer](T value, S s):
    if reflect.type_info(T).kind.name == "enum":
        # An enum serializes as its variant tag followed by any payload values.
        match value:
            case comptime v in reflect.type_info(T).variants as payload:
                s.emit_variant(comptime v.name)
                comptime for pf in v.fields:
                    if comptime pf.ty.name == "int64":
                        s.emit_int(reflect.field(payload, comptime pf.index))
                    elif comptime pf.ty.name == "uint64":
                        s.emit_uint(reflect.field(payload, comptime pf.index))
                    elif comptime pf.ty.name == "float64":
                        s.emit_float(reflect.field(payload, comptime pf.index))
                    elif comptime pf.ty.name == "str":
                        s.emit_str(reflect.field(payload, comptime pf.index))
                    elif comptime pf.ty.name == "bool":
                        s.emit_bool(reflect.field(payload, comptime pf.index))
                    elif comptime pf.ty.name == "bytes":
                        s.emit_bytes(reflect.field(payload, comptime pf.index))
                    else:
                        comptime.error(comptime pf.span, "serde.serialize: unsupported enum payload kind")
    else:
        _serialize_struct(value, s)

# A nested struct field recurses through `serialize` (the dispatcher), so a nested
# enum field is handled too. This helper is only instantiated for a struct `T`.
def _serialize_struct[T, S: Serializer](T value, S s):
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
        elif f.ty.name == "uint64":
            s.emit_uint(reflect.field(value, comptime f.index))
        elif f.ty.kind.name == "optional":
            if comptime f.ty.inner.name == "int64":
                Optional[int64] ov = reflect.field(value, comptime f.index)
                match ov:
                    case Optional.Some(x):
                        s.emit_int(x)
                    case Optional.None:
                        s.emit_nil()
            elif comptime f.ty.inner.name == "uint64":
                Optional[uint64] ov = reflect.field(value, comptime f.index)
                match ov:
                    case Optional.Some(x):
                        s.emit_uint(x)
                    case Optional.None:
                        s.emit_nil()
            elif comptime f.ty.inner.name == "float64":
                Optional[float64] ov = reflect.field(value, comptime f.index)
                match ov:
                    case Optional.Some(x):
                        s.emit_float(x)
                    case Optional.None:
                        s.emit_nil()
            elif comptime f.ty.inner.name == "str":
                Optional[str] ov = reflect.field(value, comptime f.index)
                match ov:
                    case Optional.Some(x):
                        s.emit_str(x)
                    case Optional.None:
                        s.emit_nil()
            elif comptime f.ty.inner.name == "bool":
                Optional[bool] ov = reflect.field(value, comptime f.index)
                match ov:
                    case Optional.Some(x):
                        s.emit_bool(x)
                    case Optional.None:
                        s.emit_nil()
            else:
                comptime.error(comptime f.span, "serde.serialize: unsupported optional inner kind")
        elif f.ty.kind.name == "list":
            s.begin_seq()
            for el in reflect.field(value, comptime f.index):
                if comptime f.ty.elem.name == "int64":
                    s.emit_int(el)
                elif comptime f.ty.elem.name == "uint64":
                    s.emit_uint(el)
                elif comptime f.ty.elem.name == "float64":
                    s.emit_float(el)
                elif comptime f.ty.elem.name == "str":
                    s.emit_str(el)
                elif comptime f.ty.elem.name == "bool":
                    s.emit_bool(el)
                else:
                    comptime.error(comptime f.span, "serde.serialize: unsupported list element kind")
            s.end_seq()
        elif f.ty.kind.name == "dict":
            s.begin_map()
            auto dm = reflect.field(value, comptime f.index)
            for k in dm:
                s.emit_str(k)
                if comptime f.ty.value.name == "int64":
                    s.emit_int(dm[k])
                elif comptime f.ty.value.name == "uint64":
                    s.emit_uint(dm[k])
                elif comptime f.ty.value.name == "float64":
                    s.emit_float(dm[k])
                elif comptime f.ty.value.name == "str":
                    s.emit_str(dm[k])
                elif comptime f.ty.value.name == "bool":
                    s.emit_bool(dm[k])
                else:
                    comptime.error(comptime f.span, "serde.serialize: unsupported dict value kind")
            s.end_map()
        elif f.ty.kind.name == "struct":
            serialize(reflect.field(value, comptime f.index), s)
        else:
            comptime.error(f.span, "serde.serialize: unsupported field kind")
    s.end_struct()

# Deserialize a value of struct or enum type `T` from the backend `d`. Both type
# arguments are explicit at the call site (`serde.deserialize[T, D](d)`): `T` is
# the construct target and cannot be inferred. A codec library typically wraps
# this as `unpack[T](bytes) -> T`. Enum decode this release is payload-free
# (constant) variants; a payload-carrying variant is a clear compile error.
def deserialize[T, D: Deserializer](D d) -> T:
    if reflect.type_info(T).kind.name == "enum":
        auto tag = d.read_variant()
        comptime for v in reflect.type_info(T).variants:
            comptime for pf in v.fields:
                comptime.error(comptime pf.span, "serde.deserialize: payload-carrying enum variants are not supported yet (#1561 payload decode)")
            auto vn = comptime v.name
            if tag == vn:
                return reflect.construct_variant(T, comptime v.index)
        raise ValueError("serde.deserialize: unknown enum variant '" + tag + "'")
    else:
        return _deserialize_struct[T, D](d)

# Decode a struct `T`. Only instantiated for a struct (the enum arm returns first).
def _deserialize_struct[T, D: Deserializer](D d) -> T:
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
        elif f.ty.name == "uint64":
            reflect.set_field(out, comptime f.index, d.read_uint(comptime f.name))
        elif f.ty.kind.name == "optional":
            if comptime f.ty.inner.name == "int64":
                if d.begin_optional(comptime f.name):
                    Optional[int64] sv = Optional.Some(d.read_int(comptime f.name))
                    reflect.set_field(out, comptime f.index, sv)
                else:
                    Optional[int64] nv = Optional.None
                    reflect.set_field(out, comptime f.index, nv)
            elif comptime f.ty.inner.name == "uint64":
                if d.begin_optional(comptime f.name):
                    Optional[uint64] sv = Optional.Some(d.read_uint(comptime f.name))
                    reflect.set_field(out, comptime f.index, sv)
                else:
                    Optional[uint64] nv = Optional.None
                    reflect.set_field(out, comptime f.index, nv)
            elif comptime f.ty.inner.name == "float64":
                if d.begin_optional(comptime f.name):
                    Optional[float64] sv = Optional.Some(d.read_float(comptime f.name))
                    reflect.set_field(out, comptime f.index, sv)
                else:
                    Optional[float64] nv = Optional.None
                    reflect.set_field(out, comptime f.index, nv)
            elif comptime f.ty.inner.name == "str":
                if d.begin_optional(comptime f.name):
                    Optional[str] sv = Optional.Some(d.read_str(comptime f.name))
                    reflect.set_field(out, comptime f.index, sv)
                else:
                    Optional[str] nv = Optional.None
                    reflect.set_field(out, comptime f.index, nv)
            elif comptime f.ty.inner.name == "bool":
                if d.begin_optional(comptime f.name):
                    Optional[bool] sv = Optional.Some(d.read_bool(comptime f.name))
                    reflect.set_field(out, comptime f.index, sv)
                else:
                    Optional[bool] nv = Optional.None
                    reflect.set_field(out, comptime f.index, nv)
            else:
                comptime.error(comptime f.span, "serde.deserialize: unsupported optional inner kind")
        elif f.ty.kind.name == "list":
            if comptime f.ty.elem.name == "int64":
                list[int64] xs = []
                auto n = d.begin_seq(comptime f.name)
                for _ in range(n):
                    xs.append(d.read_int(comptime f.name))
                d.end_seq()
                reflect.set_field(out, comptime f.index, xs)
            elif comptime f.ty.elem.name == "uint64":
                list[uint64] xs = []
                auto n = d.begin_seq(comptime f.name)
                for _ in range(n):
                    xs.append(d.read_uint(comptime f.name))
                d.end_seq()
                reflect.set_field(out, comptime f.index, xs)
            elif comptime f.ty.elem.name == "float64":
                list[float64] xs = []
                auto n = d.begin_seq(comptime f.name)
                for _ in range(n):
                    xs.append(d.read_float(comptime f.name))
                d.end_seq()
                reflect.set_field(out, comptime f.index, xs)
            elif comptime f.ty.elem.name == "str":
                list[str] xs = []
                auto n = d.begin_seq(comptime f.name)
                for _ in range(n):
                    xs.append(d.read_str(comptime f.name))
                d.end_seq()
                reflect.set_field(out, comptime f.index, xs)
            elif comptime f.ty.elem.name == "bool":
                list[bool] xs = []
                auto n = d.begin_seq(comptime f.name)
                for _ in range(n):
                    xs.append(d.read_bool(comptime f.name))
                d.end_seq()
                reflect.set_field(out, comptime f.index, xs)
            else:
                comptime.error(comptime f.span, "serde.deserialize: unsupported list element kind")
        elif f.ty.kind.name == "dict":
            if comptime f.ty.value.name == "int64":
                dict[str, int64] dm = {}
                auto n = d.begin_map(comptime f.name)
                for _ in range(n):
                    auto k = d.read_str(comptime f.name)
                    dm[k] = d.read_int(comptime f.name)
                d.end_map()
                reflect.set_field(out, comptime f.index, dm)
            elif comptime f.ty.value.name == "float64":
                dict[str, float64] dm = {}
                auto n = d.begin_map(comptime f.name)
                for _ in range(n):
                    auto k = d.read_str(comptime f.name)
                    dm[k] = d.read_float(comptime f.name)
                d.end_map()
                reflect.set_field(out, comptime f.index, dm)
            elif comptime f.ty.value.name == "str":
                dict[str, str] dm = {}
                auto n = d.begin_map(comptime f.name)
                for _ in range(n):
                    auto k = d.read_str(comptime f.name)
                    dm[k] = d.read_str(comptime f.name)
                d.end_map()
                reflect.set_field(out, comptime f.index, dm)
            elif comptime f.ty.value.name == "bool":
                dict[str, bool] dm = {}
                auto n = d.begin_map(comptime f.name)
                for _ in range(n):
                    auto k = d.read_str(comptime f.name)
                    dm[k] = d.read_bool(comptime f.name)
                d.end_map()
                reflect.set_field(out, comptime f.index, dm)
            else:
                comptime.error(comptime f.span, "serde.deserialize: unsupported dict value kind")
        else:
            comptime.error(f.span, "serde.deserialize: unsupported field kind")
    d.end_struct()
    return out
