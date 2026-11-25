# E4 (#1429/#1484, widened by #1490): shadow-prove that the built-in
# `@derive(Yaml)` codec can be expressed as plain library `@derivable`
# generators, WITHOUT replacing the built-in. This is the third and last E4
# *codec* slice, after Json (#1470) and Csv (#1480).
#
# Yaml is the closest sibling of Json: YAML 1.2 is a JSON superset, so the public
# `yaml` module reuses the shared `JsonValue` DOM — `yaml.load(str) -> JsonValue`
# and `yaml.dump(JsonValue) -> str` — exactly as `json.loads` / `json.dumps` do
# (goldens `yaml_dump_load.zap`, `yaml_flow.zap`). The built-in `from_yaml` even
# decodes through the same `JsonValue` core as `@derive(Json)`. So this shadow is
# the Json shadow with `json.dumps`/`json.loads` swapped for `yaml.dump`/`yaml.load`.
#
# A struct derives BOTH the compiler built-in `Yaml` (used through `to_yaml` /
# `from_yaml`) and two library codec traits, `YamlEncode` (an instance
# `to_yamlcodec(self) -> str`) and `YamlDecode` (an associated
# `from_yamlcodec(str) -> Self`). They express the same codec over the
# source-accessible YAML DOM and agree with the built-in by ROUND-TRIP — the
# agreed proof. Byte-identical encode is NOT the goal: `JsonValue.Object` is an
# unordered dict, so key (line) order is unspecified and round-trip is
# order-insensitive.
#
# #1490 widened `reflect.construct` so it also zero-constructs a nested-struct
# field (previously a clean "unsupported yet" error). With that the decoder gains
# its nested-struct branch, so a nested derived struct now round-trips through the
# LIBRARY decoder (before #1490 it was only cross-checked through the built-in
# decoder, decode-via-library deferred). `Optional` / `list` / `dict` fields stay
# out of scope here: the built-in `@derive(Yaml)` cannot codegen an `Optional`
# field yet (and does not support collections) — a separate documented follow-up
# — so there is no built-in codec to agree with for those kinds.
# `reflect.construct`'s own zero set for `Optional` / collection / tuple fields
# (the heart of #1490) is proven directly by the `construct_zero_set` golden.
#
# Built-in Yaml codegen (`gen_yaml_to_method` / `gen_yaml_from_method`), the
# `DERIVABLE_TRAITS` registry and the prelude are all untouched. Covers int / str
# / bool / float fields and a nested derived struct, both directions through the
# library.

# --- YamlEncode: build a `JsonValue.Object` from the fields, then `yaml.dump`.
# A nested derived struct recurses via its own `to_yamlcodec()`, re-parsed with
# `yaml.load` so it nests as structure (an object), not as a quoted string.
@derivable(YamlEncode)
def _derive_yamlencode[T](T value) -> str:
    dict[str, JsonValue] m = {}
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == "struct":
            m[f.name] = yaml.load(reflect.field(value, comptime f.index).to_yamlcodec())
        elif f.ty.name == "int64":
            m[f.name] = JsonValue.Int(reflect.field(value, comptime f.index))
        elif f.ty.name == "str":
            m[f.name] = JsonValue.Str(reflect.field(value, comptime f.index))
        elif f.ty.name == "bool":
            m[f.name] = JsonValue.Bool(reflect.field(value, comptime f.index))
        elif f.ty.name == "float64":
            m[f.name] = JsonValue.Float(reflect.field(value, comptime f.index))
        else:
            comptime.error(f.span, "YamlEncode: unsupported field kind")
    return yaml.dump(JsonValue.Object(m))

interface YamlEncode:
    def to_yamlcodec(self) -> str

# Per-kind readers off the DOM — ordinary source helpers (no compiler-private
# `__yaml_*` / `__json_*` prelude functions), so the shadow is a readable,
# forkable reference. The DOM is the shared `JsonValue`, so these mirror Json's.
def _yc_int(JsonValue v) -> int64:
    match v:
        case JsonValue.Int(i):
            return i
        case _:
            return 0

def _yc_str(JsonValue v) -> str:
    match v:
        case JsonValue.Str(x):
            return x
        case _:
            return ""

def _yc_bool(JsonValue v) -> bool:
    match v:
        case JsonValue.Bool(b):
            return b
        case _:
            return False

def _yc_float(JsonValue v) -> float64:
    match v:
        case JsonValue.Float(x):
            return x
        case _:
            return 0.0

# --- YamlDecode: parse to the DOM, zero-construct a T, then write each field.
# #1490 widened `reflect.construct` to zero-construct a nested-struct field, so
# this decoder now handles a nested struct too: decode it via the nested type's
# own `from_yamlcodec()` — the same generator specialized at the field type, the
# explicit type argument inferred from the `set_field` destination.
@derivable(YamlDecode)
def _derive_yamldecode[T](str s) -> T:
    auto out = reflect.construct(T)
    dict[str, JsonValue] m = {}
    match yaml.load(s):
        case JsonValue.Object(fields):
            m = fields
        case _:
            m = {}
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == "struct":
            reflect.set_field(
                out,
                comptime f.index,
                _derive_yamldecode(yaml.dump(m[f.name])),
            )
        elif f.ty.name == "int64":
            reflect.set_field(out, comptime f.index, _yc_int(m[f.name]))
        elif f.ty.name == "str":
            reflect.set_field(out, comptime f.index, _yc_str(m[f.name]))
        elif f.ty.name == "bool":
            reflect.set_field(out, comptime f.index, _yc_bool(m[f.name]))
        elif f.ty.name == "float64":
            reflect.set_field(out, comptime f.index, _yc_float(m[f.name]))
        else:
            comptime.error(f.span, "YamlDecode: unsupported field kind")
    return out

interface YamlDecode:
    def from_yamlcodec(str s) -> Self

# A nested derived struct: built-in `Yaml` plus BOTH library codec traits, so the
# outer encode can recurse AND the outer decode can call `Inner.from_yamlcodec`.
# `Hash, Eq` make it a hashable, comparable field of `Nested` (the built-in `Eq`
# of the enclosing struct requires a nested field to be hashable).
@derive(Hash, Eq, Yaml, YamlEncode, YamlDecode)
struct Inner:
    int64 a
    str s

# All four scalar/str field kinds; derives the built-in `Yaml` plus both library
# codec traits, so every direction below is expressible.
@derive(Eq, Yaml, YamlEncode, YamlDecode)
struct Flat:
    int64 x
    str s
    bool b
    float64 f

# A nested derived struct field; both the library encode (recurses) and the
# library decode (#1490) now handle it, so `Nested` derives YamlDecode too.
@derive(Eq, Yaml, YamlEncode, YamlDecode)
struct Nested:
    int64 n
    Inner inner

def main():
    auto p = Flat(7, "hi", True, 2.5)
    # library encode -> library decode round-trips to the original value...
    print(Flat.from_yamlcodec(p.to_yamlcodec()) == p)
    # ...the library decode agrees with the built-in encoder...
    print(Flat.from_yamlcodec(p.to_yaml()) == p)
    # ...and the built-in decode agrees with the library encoder.
    print(Flat.from_yaml(p.to_yamlcodec()) == p)

    auto o = Nested(3, Inner(9, "z"))
    # #1490: the nested-struct value now round-trips through the LIBRARY decoder
    # (previously decode-via-library was a clean "unsupported yet" error, and this
    # case was cross-checked only through the built-in decoder).
    print(Nested.from_yamlcodec(o.to_yamlcodec()) == o)
    # The library decode agrees with the built-in encoder for the nested value...
    print(Nested.from_yamlcodec(o.to_yaml()) == o)
    # ...the built-in decode agrees with the library encoder (holds since #1484)...
    print(Nested.from_yaml(o.to_yamlcodec()) == o)
    # ...and the built-in's own nested round-trip still holds (control).
    print(Nested.from_yaml(o.to_yaml()) == o)
