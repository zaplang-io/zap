# E5 (#1551/#1556): library `@derive(Json)` ENCODE for the AWS open string enum
# (`@json(open)`, ticket #346, D-0086), byte-identical to the built-in
# (`gen_open_enum_json` encode, `crates/ir/src/lower.rs`).
#
# The open-enum wire form: a known (payload-free) variant encodes as the JSON
# string of its NAME (`Vanilla` -> `"Vanilla"`); the `Unknown(str value)`
# fallback encodes as the JSON string of its CARRIED value (`Unknown("Mango")` ->
# `"Mango"`), forward-compatible with AWS adding enum values.
#
# The library generator consumes the enum with the reflected-variant match form
# (#1374): `case comptime v in reflect.type_info(T).variants as payload`, which
# expands to one arm per variant. A payload-free variant (`len(v.fields) == 0`)
# emits its comptime name; the payload-carrying `Unknown` emits its payload via
# `reflect.field(payload, 0)`. Each is wrapped by `json.dumps(JsonValue.Str(...))`
# so the quoting matches the built-in `zap_json_escape`.
#
# SCOPE — ENCODE ONLY. Open-enum DECODE (`from_json`: a JSON string -> a known
# variant, else `Unknown(raw)`) needs to CONSTRUCT an enum variant from the
# generator's type parameter `T` — `reflect.construct(T)` rejects enums (ZAP6001)
# and `T.Variant(...)` is unresolved (ZAP3001). That is the core gap #1561
# (filed under #1248: "generalize reflect.construct to enum + generic-struct
# instances"); enum decode is blocked on it and is NOT attempted here (no
# workaround, per the golden rule). The built-in enum decode stays the oracle and
# is untouched; the retire slice (#1558) cannot retire enum `from_json` until
# #1561 lands.
#
# The proof is a runtime string equality `e.to_json() == e.to_jsonbytes()`, gated
# by the CI LLVM-18 matrix (Docker/LLVM down; `zap ir` proves lowering only). The
# built-in Json codegen, `DERIVABLE_TRAITS` and the prelude are untouched.

@derivable(JsonBytes)
def _derive_jsonbytes[T](T value) -> str:
    str out = "null"
    match value:
        case comptime v in reflect.type_info(T).variants as payload:
            if len(v.fields) == 0:
                out = json.dumps(JsonValue.Str(v.name))
            else:
                out = json.dumps(JsonValue.Str(reflect.field(payload, comptime 0)))
    return out

interface JsonBytes:
    def to_jsonbytes(self) -> str

# An AWS open string enum: two known payload-free variants + the `Unknown(str)`
# fallback (the shape the built-in `@json(open)` requires).
@derive(Json, JsonBytes)
@json(open)
enum Flavor:
    Vanilla
    Chocolate
    Unknown(str value)

def main():
    # A known variant encodes as the JSON string of its name, byte-identical.
    auto van = Flavor.Vanilla
    print(van.to_json() == van.to_jsonbytes())
    auto cho = Flavor.Chocolate
    print(cho.to_json() == cho.to_jsonbytes())

    # The `Unknown` fallback encodes as the JSON string of its carried value.
    auto other = Flavor.Unknown(value="Strawberry")
    print(other.to_json() == other.to_jsonbytes())
    # An empty carried value (edge case) is byte-identical too.
    auto empty = Flavor.Unknown(value="")
    print(empty.to_json() == empty.to_jsonbytes())
