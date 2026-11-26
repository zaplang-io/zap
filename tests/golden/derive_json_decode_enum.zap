# E5 / #1561 / #1556: library `@derive(Json)` DECODE for the AWS open string
# enum (`@json(open)`), agreeing with the built-in. This is the decode half the
# enum slice (#1556) had to defer: a decoder must CONSTRUCT the enum variant it
# selected from a JSON string, which `reflect.construct_variant(T, comptime
# v.index, payload…)` (#1561) now supplies — the enum counterpart of
# `reflect.construct`.
#
# Open-enum decode contract (matching `gen_open_enum_json` / `json_aws_open_enum`):
# a JSON string naming a known payload-free variant → that variant; any other
# string → the `Unknown(str)` fallback carrying the raw string. The generator
# finds the match in a `comptime for` over `reflect.type_info(T).variants` and
# builds the chosen variant by its comptime index.
#
# Proof is AGREEMENT with the built-in decoder, by re-encoding the decoded value
# with the built-in `to_json()` (gated by the CI LLVM-18 matrix; `zap ir` proves
# lowering only). The built-in codec is untouched.

@derivable(JsonEnumDecode)
def _derive_jsonenumdecode[T](str s) -> T:
    # `s` is the JSON string's text (the decoded scalar); match it against each
    # known (payload-free) variant's name.
    comptime for v in reflect.type_info(T).variants:
        if len(v.fields) == 0:
            if s == v.name:
                return reflect.construct_variant(T, comptime v.index)
    # No known name matched — build the `Unknown(str)` fallback (the one variant
    # carrying a single payload field) with the raw string.
    comptime for v in reflect.type_info(T).variants:
        if len(v.fields) == 1:
            return reflect.construct_variant(T, comptime v.index, s)
    # A valid open enum always has the fallback; this keeps the function total.
    return reflect.construct_variant(T, comptime 0)

interface JsonEnumDecode:
    def from_jsonenum(str s) -> Self

@derive(Json, JsonEnumDecode)
@json(open)
enum Flavor:
    Vanilla
    Chocolate
    Unknown(str value)

def main():
    # A known name decodes to its variant (re-encodes to the quoted name).
    print(Flavor.from_jsonenum("Vanilla").to_json() == "\"Vanilla\"")
    print(Flavor.from_jsonenum("Chocolate").to_json() == "\"Chocolate\"")
    # An unknown name decodes to the fallback carrying the raw string.
    print(Flavor.from_jsonenum("Mango").to_json() == "\"Mango\"")
    # Agreement with the built-in decoder on the same (quoted) JSON input.
    print(
        Flavor.from_jsonenum("Vanilla").to_json() == Flavor.from_json("\"Vanilla\"").to_json(),
    )
    print(
        Flavor.from_jsonenum("Mango").to_json() == Flavor.from_json("\"Mango\"").to_json(),
    )
