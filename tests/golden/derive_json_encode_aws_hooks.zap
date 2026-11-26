# E5 (#1551/#1555): library `@derive(Json)` ENCODE for the AWS `@json` field
# hooks (ticket #346, D-0086), byte-identical to the built-in (`gen_json_encode_field`
# bytes / DateTime branches, `crates/ir/src/lower.rs`). These hooks feed the
# generated AWS SDK, so retiring the built-in (#1558) must reproduce them exactly.
#
#   * `bytes` -> base64 (standard alphabet, padded) JSON string. Built-in:
#     `zap_base64_encode(v, 0, 1)` + `zap_json_escape`. Library:
#     `json.dumps(JsonValue.Str(base64.encode(v)))` (base64.encode is standard +
#     padded). base64 output is ASCII, so no escaping differs from the built-in.
#   * `DateTime` -> epoch seconds (JSON number, the default) or ISO-8601 (JSON
#     string), selected per field by `@json(timestamp=...)`. Built-in:
#     `zap_datetime_to_unix` / `zap_datetime_format_iso`. Library:
#     `datetime.to_timestamp(dt)` (-> int64) and one-arg `datetime.format(dt)`
#     (-> `2023-11-14T22:13:20`), the same runtime paths. The timestamp form is
#     read off the field's `@json` decorator via comptime `f.attrs`.
#
# The proof is a runtime string equality `s.to_json() == s.to_jsonbytes()`, gated
# by the CI LLVM-18 matrix (Docker/LLVM down; `zap ir` proves lowering only). The
# built-in codegen, `DERIVABLE_TRAITS` and the prelude are untouched.

@derivable(JsonBytes)
def _derive_jsonbytes[T](T value) -> str:
    list[str] parts = []
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == "bytes":
            # AWS blob hook: base64 (standard, padded) as a JSON string.
            parts.append(
                "\"" + f.name + "\":" + json.dumps(JsonValue.Str(base64.encode(reflect.field(value, comptime f.index)))),
            )
        elif f.ty.name == "DateTime":
            # AWS timestamp hook: epoch seconds (default) or ISO-8601 per the
            # field's `@json(timestamp=...)`, read off `f.attrs` at comptime.
            auto dt = reflect.field(value, comptime f.index)
            str tsfmt = "epoch"
            comptime for a in f.attrs:
                if a.name == "json":
                    comptime for arg in a.args:
                        if arg.is_named:
                            if arg.name == "timestamp":
                                tsfmt = arg.value
            if tsfmt == "iso8601":
                parts.append(
                    "\"" + f.name + "\":" + json.dumps(JsonValue.Str(datetime.format(dt))),
                )
            else:
                parts.append(
                    "\"" + f.name + "\":" + json.dumps(JsonValue.Int(datetime.to_timestamp(dt))),
                )
        elif f.ty.kind.name == "struct":
            parts.append(
                "\"" + f.name + "\":" + reflect.field(value, comptime f.index).to_jsonbytes(),
            )
        elif f.ty.name == "int64":
            parts.append(
                "\"" + f.name + "\":" + json.dumps(JsonValue.Int(reflect.field(value, comptime f.index))),
            )
        elif f.ty.name == "str":
            parts.append(
                "\"" + f.name + "\":" + json.dumps(JsonValue.Str(reflect.field(value, comptime f.index))),
            )
        elif f.ty.name == "bool":
            parts.append(
                "\"" + f.name + "\":" + json.dumps(JsonValue.Bool(reflect.field(value, comptime f.index))),
            )
        elif f.ty.name == "float64":
            parts.append(
                "\"" + f.name + "\":" + json.dumps(JsonValue.Float(reflect.field(value, comptime f.index))),
            )
        else:
            comptime.error(f.span, "JsonBytes: unsupported field kind")
    return "{" + ",".join(parts) + "}"

interface JsonBytes:
    def to_jsonbytes(self) -> str

# A `bytes` (blob) field.
@derive(Json, JsonBytes)
struct Blob:
    bytes data

# A DateTime field defaulting to epoch, and one selected as ISO-8601, alongside a
# plain `str` field — the shape the AWS SDK uses.
@derive(Json, JsonBytes)
struct Event:
    str name
    DateTime at
    @json(timestamp="iso8601")
    DateTime seen

def main():
    # --- blob: base64 string, byte-identical.
    print(Blob(data=b"foobar").to_json() == Blob(data=b"foobar").to_jsonbytes())
    # Empty bytes and binary (non-UTF-8) bytes byte-identical too.
    print(Blob(data=b"").to_json() == Blob(data=b"").to_jsonbytes())
    auto bin = Blob(data=bytes([251, 255]))
    print(bin.to_json() == bin.to_jsonbytes())

    # --- timestamp: epoch (default) + ISO-8601 (per @json), byte-identical. A
    # FIXED timestamp keeps it deterministic/UTC: 1700000000 = 2023-11-14T22:13:20Z.
    auto dt = datetime.from_timestamp(1700000000)
    auto e = Event(name="boot", at=dt, seen=dt)
    print(e.to_json() == e.to_jsonbytes())
    # A second instant pins a different value through both forms.
    auto dt2 = datetime.from_timestamp(0)
    auto e2 = Event(name="epoch", at=dt2, seen=dt2)
    print(e2.to_json() == e2.to_jsonbytes())
