# Built-in `@derive(Json)`, as a library `@derivable` codec (E5, #1558, epic
# #1551). `@derive(Json)` is sugar for deriving both `__JsonDeriveEncode` (an instance
# `to_json(self) -> str`) and `__JsonDeriveDecode` (an associated `from_json(str) ->
# Self`), each a single-method trait with its own generator. The generators are
# plain Zap over the source-accessible JSON DOM (`JsonValue`, `json.dumps` /
# `json.loads`) plus `reflect.*`; they replace the retired compiler codegen
# (`gen_json_*` / `gen_open_enum_json`) and reproduce it byte-for-byte.
#
# Injected into a module only when it routes a `@derive(Json)` (#1596), so a
# program that does not derive Json is unaffected.

interface __JsonDeriveEncode:
    def to_json(self) -> str

interface __JsonDeriveDecode:
    def from_json(str s) -> Self

# --- DOM readers (plain helpers; no comptime, so they specialize normally).
def __json_rd_int(JsonValue v) -> int64:
    match v:
        case JsonValue.Int(i):
            return i
        case _:
            return 0

def __json_rd_str(JsonValue v) -> str:
    match v:
        case JsonValue.Str(x):
            return x
        case _:
            return ""

def __json_rd_bool(JsonValue v) -> bool:
    match v:
        case JsonValue.Bool(b):
            return b
        case _:
            return False

def __json_rd_float(JsonValue v) -> float64:
    match v:
        case JsonValue.Float(x):
            return x
        case _:
            return 0.0

def __json_get(dict[str, JsonValue] m, str k) -> JsonValue:
    if k in m:
        return m[k]
    return JsonValue.Null

# --- Recursive element codec (#1604/#1608): a scalar leaf, a nested derived
# struct / enum (via its own `to_json` / `from_json`), or a `list` / `dict` that
# recurses into its element / value type named through `reflect.type_info(E)`.
def __json_enc_elem[E](E x) -> str:
    if reflect.type_info(E).name == "int64":
        return json.dumps(JsonValue.Int(x))
    elif reflect.type_info(E).name == "str":
        return json.dumps(JsonValue.Str(x))
    elif reflect.type_info(E).name == "bool":
        return json.dumps(JsonValue.Bool(x))
    elif reflect.type_info(E).name == "float64":
        return json.dumps(JsonValue.Float(x))
    elif reflect.type_info(E).kind.name == "list":
        list[str] parts = []
        for y in x:
            parts.append(__json_enc_elem[reflect.type_info(E).elem](y))
        return "[" + ",".join(parts) + "]"
    elif reflect.type_info(E).kind.name == "dict":
        list[str] entries = []
        for k, v in x.items():
            entries.append(json.dumps(JsonValue.Str(k)) + ":" + __json_enc_elem[reflect.type_info(E).value](v))
        return "{" + ",".join(entries) + "}"
    else:
        return x.to_json()

def __json_dec_elem[E](JsonValue v) -> E:
    if reflect.type_info(E).name == "int64":
        return __json_rd_int(v)
    elif reflect.type_info(E).name == "str":
        return __json_rd_str(v)
    elif reflect.type_info(E).name == "bool":
        return __json_rd_bool(v)
    elif reflect.type_info(E).name == "float64":
        return __json_rd_float(v)
    elif reflect.type_info(E).kind.name == "list":
        auto acc = reflect.construct(E)
        match v:
            case JsonValue.Array(xs):
                for jv in xs:
                    acc.append(__json_dec_elem[reflect.type_info(E).elem](jv))
            case _:
                pass
        return acc
    elif reflect.type_info(E).kind.name == "dict":
        auto dacc = reflect.construct(E)
        match v:
            case JsonValue.Object(fs):
                for k in fs.keys():
                    dacc[k] = __json_dec_elem[reflect.type_info(E).value](fs[k])
            case _:
                pass
        return dacc
    else:
        return __derive_json_from[E](json.dumps(v))

# --- Encode: a struct to a field-ordered JSON object, or an open string enum to
# its name / carried value. Byte-identical to the retired built-in.
@derivable(__JsonDeriveEncode)
def __derive_json_to[T](T value) -> str:
    if reflect.type_info(T).kind.name == "enum":
        str out = "null"
        match value:
            case comptime vv in reflect.type_info(T).variants as payload:
                if len(vv.fields) == 0:
                    out = json.dumps(JsonValue.Str(vv.name))
                else:
                    out = json.dumps(JsonValue.Str(reflect.field(payload, comptime 0)))
        return out
    else:
        list[str] parts = []
        comptime for f in reflect.type_info(T).fields:
            if f.ty.kind.name == "optional":
                auto ov = reflect.field(value, comptime f.index)
                if ov.is_some():
                    parts.append("\"" + f.name + "\":" + __json_enc_elem[f.ty.inner](ov.unwrap()))
            elif f.ty.name == "bytes":
                parts.append("\"" + f.name + "\":" + json.dumps(JsonValue.Str(base64.encode(reflect.field(value, comptime f.index)))))
            elif f.ty.name == "DateTime":
                auto dt = reflect.field(value, comptime f.index)
                str tsfmt = "epoch"
                comptime for a in f.attrs:
                    if a.name == "json":
                        comptime for arg in a.args:
                            if arg.is_named:
                                if arg.name == "timestamp":
                                    tsfmt = arg.value
                if tsfmt == "iso8601":
                    parts.append("\"" + f.name + "\":" + json.dumps(JsonValue.Str(datetime.format(dt))))
                else:
                    parts.append("\"" + f.name + "\":" + json.dumps(JsonValue.Int(datetime.to_timestamp(dt))))
            else:
                parts.append("\"" + f.name + "\":" + __json_enc_elem[f.ty](reflect.field(value, comptime f.index)))
        return "{" + ",".join(parts) + "}"

# --- Decode: a JSON document to a struct (zero-construct + per-field set), or an
# open string enum (match the JSON string to a known variant, else Unknown).
@derivable(__JsonDeriveDecode)
def __derive_json_from[T](str s) -> T:
    if reflect.type_info(T).kind.name == "enum":
        str nm = ""
        match json.loads(s):
            case JsonValue.Str(x):
                nm = x
            case _:
                nm = ""
        comptime for v in reflect.type_info(T).variants:
            if len(v.fields) == 0:
                if nm == v.name:
                    return reflect.construct_variant(T, comptime v.index)
        comptime for v in reflect.type_info(T).variants:
            if len(v.fields) == 1:
                return reflect.construct_variant(T, comptime v.index, nm)
        return reflect.construct_variant(T, comptime 0)
    else:
        auto out = reflect.construct(T)
        dict[str, JsonValue] m = {}
        match json.loads(s):
            case JsonValue.Object(fields):
                m = fields
            case _:
                m = {}
        comptime for f in reflect.type_info(T).fields:
            if f.ty.kind.name == "optional":
                if f.name in m:
                    if f.ty.inner.name == "int64":
                        match m[f.name]:
                            case JsonValue.Int(i):
                                reflect.set_field(out, comptime f.index, Optional.Some(i))
                            case _:
                                pass
                    elif f.ty.inner.name == "str":
                        match m[f.name]:
                            case JsonValue.Str(x):
                                reflect.set_field(out, comptime f.index, Optional.Some(x))
                            case _:
                                pass
                    else:
                        comptime.error(f.span, "@derive(Json): unsupported Optional payload")
            elif f.ty.name == "bytes":
                reflect.set_field(out, comptime f.index, base64.decode(__json_rd_str(__json_get(m, f.name))))
            elif f.ty.name == "DateTime":
                str tsfmt = "epoch"
                comptime for a in f.attrs:
                    if a.name == "json":
                        comptime for arg in a.args:
                            if arg.is_named:
                                if arg.name == "timestamp":
                                    tsfmt = arg.value
                if tsfmt == "iso8601":
                    reflect.set_field(out, comptime f.index, datetime.parse(__json_rd_str(__json_get(m, f.name))))
                else:
                    reflect.set_field(out, comptime f.index, datetime.from_timestamp(__json_rd_int(__json_get(m, f.name))))
            else:
                reflect.set_field(out, comptime f.index, __json_dec_elem[f.ty](__json_get(m, f.name)))
        return out
