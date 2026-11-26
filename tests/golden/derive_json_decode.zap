# E5 (#1551/#1557): library `@derive(Json)` DECODE — `from_json(str) -> Self`
# over the JSON DOM, agreeing with the built-in `@derive(Json)` decode
# (`gen_json_from_method` / `gen_json_decode_*`, `crates/ir/src/lower.rs`). Builds
# on the E4 shadow decode (#1470/#1490/#1522, scalars/str/bool/float/nested/
# Optional) and extends it to scalar `list` / `dict` fields and the AWS hooks,
# with agreement to the built-in as the gate.
#
# Mechanism: parse with `json.loads`, zero-construct with `reflect.construct(T)`,
# write each field with `reflect.set_field(out, comptime f.index, …)`, reading
# each field's `JsonValue` off the decoded object (no compiler-private helpers).
#
# SCOPE — struct decode. Covered: int64 / str / bool / float64, a nested derived
# struct FIELD (recurses via the generator, T inferred from the set_field
# destination, #1490), an `Optional[int64]` (absent key or explicit null -> None,
# D-0065), `list[scalar]` / `dict[str, scalar]`, and the AWS hooks (base64 JSON
# string -> bytes; JSON number/string -> DateTime per `@json(timestamp=)`).
#
# NOT COVERED (blocked, reported not worked around): a nested *derived struct*
# as a LIST element or DICT value (`list[Item]` / `dict[str, Item]`). The element
# decoder must be invoked for a comptime-known element type with no `set_field`
# destination to anchor inference, and an explicit type-argument call
# (`_derive_...[f.ty.elem](…)`) does not resolve inside a specialized derive
# generator body. Filed as a core gap under #1248. Enum decode and generic-struct
# decode remain blocked on #1561. The built-in decode stays the oracle and is
# untouched; the retire slice (#1558) defers these cases accordingly.
#
# Proof: agreement with the built-in — `T.from_json(v.to_json()) ==` the original
# (Eq structs) or keyed/indexed inspection (collection / hook structs). Gated by
# the CI LLVM-18 matrix (Docker/LLVM down; `zap ir` proves lowering only).

# --- Per-kind readers off the DOM (ordinary source helpers).
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

# The JSON array at key `k`, or `[]` (absent / wrong kind).
def _jc_arr(dict[str, JsonValue] m, str k) -> list[JsonValue]:
    if k in m:
        match m[k]:
            case JsonValue.Array(xs):
                return xs
            case _:
                return []
    return []

# The JSON object at key `k`, or `{}` (absent / wrong kind).
def _jc_obj(dict[str, JsonValue] m, str k) -> dict[str, JsonValue]:
    if k in m:
        match m[k]:
            case JsonValue.Object(fs):
                return fs
            case _:
                return {}
    return {}

@derivable(JsonDecodeB)
def _derive_jsondecodeb[T](str s) -> T:
    auto out = reflect.construct(T)
    dict[str, JsonValue] m = {}
    match json.loads(s):
        case JsonValue.Object(fields):
            m = fields
        case _:
            m = {}
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == "optional":
            # Null-safe (D-0065): absent key OR explicit null -> None, else Some.
            # Checked before the container arms: an `Optional` must not fall into
            # the `list` arm's element dispatch.
            Optional[int64] dv = None
            if f.name in m:
                match m[f.name]:
                    case JsonValue.Int(i):
                        dv = Optional.Some(i)
                    case _:
                        dv = None
            reflect.set_field(out, comptime f.index, dv)
        elif f.ty.kind.name == "list":
            # `list[scalar]`: decode each element off the JSON array into the
            # zero-constructed (empty) list field, which reference-populates `out`.
            auto acc = reflect.field(out, comptime f.index)
            for jv in _jc_arr(m, f.name):
                if f.ty.elem.name == "int64":
                    acc.append(_jc_int(jv))
                elif f.ty.elem.name == "str":
                    acc.append(_jc_str(jv))
                elif f.ty.elem.name == "bool":
                    acc.append(_jc_bool(jv))
                elif f.ty.elem.name == "float64":
                    acc.append(_jc_float(jv))
                else:
                    comptime.error(f.span, "JsonDecodeB: unsupported list element kind")
            reflect.set_field(out, comptime f.index, acc)
        elif f.ty.kind.name == "dict":
            # `dict[str, scalar]`: decode each value off the JSON object.
            auto dacc = reflect.field(out, comptime f.index)
            auto dm = _jc_obj(m, f.name)
            for k in dm.keys():
                if f.ty.value.name == "int64":
                    dacc[k] = _jc_int(dm[k])
                elif f.ty.value.name == "str":
                    dacc[k] = _jc_str(dm[k])
                elif f.ty.value.name == "bool":
                    dacc[k] = _jc_bool(dm[k])
                elif f.ty.value.name == "float64":
                    dacc[k] = _jc_float(dm[k])
                else:
                    comptime.error(f.span, "JsonDecodeB: unsupported dict value kind")
            reflect.set_field(out, comptime f.index, dacc)
        elif f.ty.name == "bytes":
            # AWS blob hook: base64 JSON string -> bytes.
            reflect.set_field(out, comptime f.index, base64.decode(_jc_str(m[f.name])))
        elif f.ty.name == "DateTime":
            # AWS timestamp hook: epoch number (default) or ISO-8601 string per
            # the field's `@json(timestamp=...)`, read off `f.attrs`. Checked
            # before the generic struct arm since DateTime is a struct kind.
            str tsfmt = "epoch"
            comptime for a in f.attrs:
                if a.name == "json":
                    comptime for arg in a.args:
                        if arg.is_named:
                            if arg.name == "timestamp":
                                tsfmt = arg.value
            if tsfmt == "iso8601":
                reflect.set_field(
                    out,
                    comptime f.index,
                    datetime.parse(_jc_str(m[f.name])),
                )
            else:
                reflect.set_field(
                    out,
                    comptime f.index,
                    datetime.from_timestamp(_jc_int(m[f.name])),
                )
        elif f.ty.kind.name == "struct":
            # A nested derived struct FIELD: recurse via the generator, T inferred
            # from the set_field destination (#1490). Re-serialize the sub-DOM.
            reflect.set_field(
                out,
                comptime f.index,
                _derive_jsondecodeb(json.dumps(m[f.name])),
            )
        elif f.ty.name == "int64":
            reflect.set_field(out, comptime f.index, _jc_int(m[f.name]))
        elif f.ty.name == "str":
            reflect.set_field(out, comptime f.index, _jc_str(m[f.name]))
        elif f.ty.name == "bool":
            reflect.set_field(out, comptime f.index, _jc_bool(m[f.name]))
        elif f.ty.name == "float64":
            reflect.set_field(out, comptime f.index, _jc_float(m[f.name]))
        else:
            comptime.error(f.span, "JsonDecodeB: unsupported field kind")
    return out

interface JsonDecodeB:
    def from_jsonb(str s) -> Self

# A nested derived struct FIELD (Hash + Eq so the enclosing Eq is derivable).
@derive(Hash, Eq, Json, JsonDecodeB)
struct Inner:
    int64 a
    str s

# All scalar/str kinds; Eq so the round-trip compares with `==`.
@derive(Eq, Json, JsonDecodeB)
struct Flat:
    int64 x
    str s
    bool b
    float64 f

# A nested struct field + an Optional scalar.
@derive(Eq, Json, JsonDecodeB)
struct Nested:
    int64 n
    Inner inner
    Optional[int64] opt

# Scalar collections (no Eq: collections are not hashable, D-0049) — inspected.
@derive(Json, JsonDecodeB)
struct Coll:
    list[int64] nums
    list[str] tags
    dict[str, int64] scores

# AWS hooks (no Eq: bytes / DateTime) — inspected.
@derive(Json, JsonDecodeB)
struct Rec:
    str name
    bytes data
    DateTime at
    @json(timestamp="iso8601")
    DateTime seen

def main():
    # --- Flat: library decode of the built-in encode round-trips to the value.
    auto f1 = Flat(7, "hi", True, 2.5)
    print(Flat.from_jsonb(f1.to_json()) == f1)
    auto f2 = Flat(-3, "a\"b", False, 0.0)
    print(Flat.from_jsonb(f2.to_json()) == f2)

    # --- Nested: nested struct field + Optional, present and absent.
    auto o = Nested(3, Inner(9, "z"), Optional.Some(5))
    print(Nested.from_jsonb(o.to_json()) == o)
    auto e = Nested(1, Inner(0, ""), None)
    print(Nested.from_jsonb(e.to_json()) == e)

    # --- Coll: scalar list / dict, inspected by index / key (not Eq).
    auto c = Coll([1, 2, 3], ["a", "b"], {"k": 5})
    auto rc = Coll.from_jsonb(c.to_json())
    print(rc.nums[0] == 1)
    print(rc.nums[2] == 3)
    print(len(rc.nums) == 3)
    print(rc.tags[1] == "b")
    print(rc.scores["k"] == 5)

    # --- Rec: AWS hooks, inspected.
    auto dt = datetime.from_timestamp(1700000000)
    auto r = Rec(name="boot", data=b"foobar", at=dt, seen=dt)
    auto rr = Rec.from_jsonb(r.to_json())
    print(rr.name == "boot")
    print(base64.encode(rr.data) == "Zm9vYmFy")
    print(datetime.to_timestamp(rr.at) == 1700000000)
    print(datetime.to_timestamp(rr.seen) == 1700000000)
