# E4 (#1429/#1470, widened by #1490): shadow-prove that the built-in
# `@derive(Json)` codec can be expressed as plain library `@derivable`
# generators, WITHOUT replacing the built-in. This is the first E4 *codec* slice,
# exercising the two enablers that landed for it: `reflect.construct` /
# `reflect.set_field` (#1454) and derived-method `Self`-return typing (#1455).
#
# A struct derives BOTH the compiler built-in `Json` (used through `to_json` /
# `from_json`) and two library codec traits, `JsonEncode` (an instance
# `to_jsoncodec(self) -> str`) and `JsonDecode` (an associated
# `from_jsoncodec(str) -> Self`). The two express the same codec over the
# source-accessible JSON DOM (`JsonValue` + public `json.dumps` / `json.loads`),
# and agree with the built-in by ROUND-TRIP — the agreed proof. Byte-identical
# encode is NOT the goal: `JsonValue.Object` is an unordered dict, so key order is
# unspecified, and round-trip is order-insensitive.
#
# #1490 widened `reflect.construct` so it also zero-constructs a nested-struct
# field, and #1522 adds the `Optional` codec branch (unblocked by #1506, which
# desugars `reflect.set_field` inside a `match` arm, and #1517, which gave the
# built-in Yaml codec `Optional` parity). So both a nested derived struct and an
# `Optional` scalar now round-trip through the LIBRARY decoder (before #1490 /
# #1522 decode-via-library was a clean "unsupported yet" error and these cases
# were cross-checked only through the built-in decoder). The `Optional` decode is
# null-safe and matches the built-in's D-0065 semantics: an absent key OR an
# explicit `null` -> `None`, a scalar -> `Some(v)`.
#
# `list` / `dict` fields stay out of scope: the built-in `@derive(Json)` does not
# support collection fields yet (a separate documented follow-up), so there is no
# built-in codec to agree with for those kinds. `reflect.construct`'s own zero set
# for collection / tuple fields (the heart of #1490) is proven directly by the
# `construct_zero_set` golden.
#
# Built-in Json codegen (`gen_json_to_method` / `gen_json_from_method`), the
# `DERIVABLE_TRAITS` registry and the prelude are all untouched. Covers int / str
# / bool / float fields, a nested derived struct, and an `Optional` scalar — both
# directions through the library.

# --- JsonEncode: build a `JsonValue.Object` from the fields, then `json.dumps`.
# A nested derived struct recurses via its own `to_jsoncodec()`, re-parsed with
# `json.loads` so it nests as structure (an object), not as a quoted string.
@derivable(JsonEncode)
def _derive_jsonencode[T](T value) -> str:
    dict[str, JsonValue] m = {}
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == "struct":
            m[f.name] = json.loads(
                reflect.field(value, comptime f.index).to_jsoncodec(),
            )
        elif f.ty.kind.name == "optional":
            # `Some(x)` -> the scalar; `None` -> `JsonValue.Null` (the built-in
            # omits a `None` key, but both decoders map an explicit null back to
            # `None`, so emitting null round-trips either way). The shadow
            # exercises `Optional[int64]`; the bound local annotates the matched
            # value's instance (I#154).
            Optional[int64] ov = reflect.field(value, comptime f.index)
            JsonValue jv = JsonValue.Null
            match ov:
                case Optional.Some(x):
                    jv = JsonValue.Int(x)
                case Optional.None:
                    jv = JsonValue.Null
            m[f.name] = jv
        elif f.ty.name == "int64":
            m[f.name] = JsonValue.Int(reflect.field(value, comptime f.index))
        elif f.ty.name == "str":
            m[f.name] = JsonValue.Str(reflect.field(value, comptime f.index))
        elif f.ty.name == "bool":
            m[f.name] = JsonValue.Bool(reflect.field(value, comptime f.index))
        elif f.ty.name == "float64":
            m[f.name] = JsonValue.Float(reflect.field(value, comptime f.index))
        else:
            comptime.error(f.span, "JsonEncode: unsupported field kind")
    return json.dumps(JsonValue.Object(m))

interface JsonEncode:
    def to_jsoncodec(self) -> str

# Per-kind readers off the DOM — ordinary source helpers (no compiler-private
# `__json_*` prelude functions), so the shadow is a readable, forkable reference.
def _jc_int(JsonValue v) -> int64:
    match v:
        case JsonValue.Int(i):
            return i
        case _:
            return 0

def _jc_str(JsonValue v) -> str:
    match v:
        case JsonValue.Str(x):
            return x
        case _:
            return ""

def _jc_bool(JsonValue v) -> bool:
    match v:
        case JsonValue.Bool(b):
            return b
        case _:
            return False

def _jc_float(JsonValue v) -> float64:
    match v:
        case JsonValue.Float(x):
            return x
        case _:
            return 0.0

# --- JsonDecode: parse to the DOM, zero-construct a T, then write each field.
# #1490 widened `reflect.construct` to zero-construct a nested-struct field, so
# this decoder now handles a nested struct too: decode it via the nested type's
# own `from_jsoncodec()` — the same generator specialized at the field type, the
# explicit type argument inferred from the `set_field` destination.
@derivable(JsonDecode)
def _derive_jsondecode[T](str s) -> T:
    auto out = reflect.construct(T)
    dict[str, JsonValue] m = {}
    match json.loads(s):
        case JsonValue.Object(fields):
            m = fields
        case _:
            m = {}
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == "struct":
            reflect.set_field(
                out,
                comptime f.index,
                _derive_jsondecode(json.dumps(m[f.name])),
            )
        elif f.ty.kind.name == "optional":
            # Null-safe decode matching the built-in (D-0065): an absent key OR an
            # explicit `JsonValue.Null` -> `None`, a scalar -> `Some(v)`. The
            # presence check (`in`) is load-bearing — the built-in omits a `None`
            # key, so a bare `m[f.name]` would read a missing entry.
            Optional[int64] dv = None
            if f.name in m:
                match m[f.name]:
                    case JsonValue.Int(i):
                        dv = Optional.Some(i)
                    case _:
                        dv = None
            reflect.set_field(out, comptime f.index, dv)
        elif f.ty.name == "int64":
            reflect.set_field(out, comptime f.index, _jc_int(m[f.name]))
        elif f.ty.name == "str":
            reflect.set_field(out, comptime f.index, _jc_str(m[f.name]))
        elif f.ty.name == "bool":
            reflect.set_field(out, comptime f.index, _jc_bool(m[f.name]))
        elif f.ty.name == "float64":
            reflect.set_field(out, comptime f.index, _jc_float(m[f.name]))
        else:
            comptime.error(f.span, "JsonDecode: unsupported field kind")
    return out

interface JsonDecode:
    def from_jsoncodec(str s) -> Self

# A nested derived struct: built-in `Json` plus BOTH library codec traits, so the
# outer encode can recurse AND the outer decode can call `Inner.from_jsoncodec`.
# `Hash, Eq` make it a hashable, comparable field of `Nested` (the built-in `Eq`
# of the enclosing struct requires a nested field to be hashable).
@derive(Hash, Eq, Json, JsonEncode, JsonDecode)
struct Inner:
    int64 a
    str s

# All four scalar/str field kinds; derives the built-in `Json` plus both library
# codec traits, so every direction below is expressible.
@derive(Eq, Json, JsonEncode, JsonDecode)
struct Flat:
    int64 x
    str s
    bool b
    float64 f

# A nested derived struct field and an `Optional` scalar; the library encode
# (recurses / maps the option) and the library decode (#1490 nested, #1522
# optional) handle both, so `Nested` derives JsonDecode too.
@derive(Eq, Json, JsonEncode, JsonDecode)
struct Nested:
    int64 n
    Inner inner
    Optional[int64] opt

def main():
    auto p = Flat(7, "hi", True, 2.5)
    # library encode -> library decode round-trips to the original value...
    print(Flat.from_jsoncodec(p.to_jsoncodec()) == p)
    # ...the library decode agrees with the built-in encoder...
    print(Flat.from_jsoncodec(p.to_json()) == p)
    # ...and the built-in decode agrees with the library encoder.
    print(Flat.from_json(p.to_jsoncodec()) == p)

    auto o = Nested(3, Inner(9, "z"), Optional.Some(5))
    # #1490: the nested-struct value round-trips through the LIBRARY decoder;
    # #1522: the `Some` optional does too.
    print(Nested.from_jsoncodec(o.to_jsoncodec()) == o)
    # The library decode agrees with the built-in encoder...
    print(Nested.from_jsoncodec(o.to_json()) == o)
    # ...the built-in decode agrees with the library encoder (holds since #1470)...
    print(Nested.from_json(o.to_jsoncodec()) == o)
    # ...and the built-in's own round-trip still holds (control).
    print(Nested.from_json(o.to_json()) == o)

    # #1522: a `None` optional round-trips through the library decoder too — the
    # built-in omits the key on encode, so the decode's presence check maps the
    # absent key back to `None`.
    auto e = Nested(1, Inner(0, ""), None)
    print(Nested.from_jsoncodec(e.to_jsoncodec()) == e)
    print(Nested.from_jsoncodec(e.to_json()) == e)
    print(Nested.from_json(e.to_jsoncodec()) == e)
