# E5 / #1575: library `@derive(Json)` DECODE reaching a nested derived struct as
# a `list` element and a `dict` value. This is the case the decode slice (#1557)
# had to defer: the element decoder must be invoked for a comptime-known element
# type with no `reflect.set_field` destination to anchor inference. #1575 lets a
# derive generator name the element type through reflection —
# `gen[f.ty.elem](...)` for a list element, `gen[f.ty.value](...)` for a dict
# value — and comptime specialization rewrites that reflected type argument to
# the concrete element type.
#
# The proof is AGREEMENT with the built-in `@derive(Json)` decoder, by keyed /
# indexed inspection (a struct with collection fields is not `Eq`-able, and dict
# order is unspecified, #1066), gated by the CI LLVM-18 matrix (Docker/LLVM down;
# `zap ir` proves lowering only). The built-in codec is untouched.

def _jc_int(JsonValue v) -> int64:
    match v:
        case JsonValue.Int(i):
            return i
        case _:
            return 0

def _jc_arr(dict[str, JsonValue] m, str k) -> list[JsonValue]:
    if k in m:
        match m[k]:
            case JsonValue.Array(xs):
                return xs
            case _:
                return []
    return []

def _jc_obj(dict[str, JsonValue] m, str k) -> dict[str, JsonValue]:
    if k in m:
        match m[k]:
            case JsonValue.Object(fs):
                return fs
            case _:
                return {}
    return {}

@derivable(JsonDecodeN)
def _derive_jsondecoden[T](str s) -> T:
    auto out = reflect.construct(T)
    dict[str, JsonValue] m = {}
    match json.loads(s):
        case JsonValue.Object(fields):
            m = fields
        case _:
            m = {}
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == "list":
            # A `list[T]` field: decode each element. A nested derived-struct
            # element is decoded by naming the element type through reflection
            # (#1575) — `_derive_jsondecoden[f.ty.elem]` — re-serializing the
            # element's sub-DOM and recursing the generator at the element type.
            auto acc = reflect.field(out, comptime f.index)
            for jv in _jc_arr(m, f.name):
                if f.ty.elem.name == "int64":
                    acc.append(_jc_int(jv))
                elif f.ty.elem.kind.name == "struct":
                    acc.append(_derive_jsondecoden[f.ty.elem](json.dumps(jv)))
                else:
                    comptime.error(f.span, "unsupported list element kind")
            reflect.set_field(out, comptime f.index, acc)
        elif f.ty.kind.name == "dict":
            # A `dict[str, V]` field: decode each value; a nested derived-struct
            # value is reached through `f.ty.value` (#1575).
            auto dacc = reflect.field(out, comptime f.index)
            auto dm = _jc_obj(m, f.name)
            for k in dm.keys():
                if f.ty.value.name == "int64":
                    dacc[k] = _jc_int(dm[k])
                elif f.ty.value.kind.name == "struct":
                    dacc[k] = _derive_jsondecoden[f.ty.value](json.dumps(dm[k]))
                else:
                    comptime.error(f.span, "unsupported dict value kind")
            reflect.set_field(out, comptime f.index, dacc)
        elif f.ty.kind.name == "struct":
            # A nested derived struct FIELD (T inferred from the set_field
            # destination, #1490) — the direct-field counterpart.
            reflect.set_field(
                out,
                comptime f.index,
                _derive_jsondecoden(json.dumps(m[f.name])),
            )
        elif f.ty.name == "int64":
            reflect.set_field(out, comptime f.index, _jc_int(m[f.name]))
        else:
            comptime.error(f.span, "unsupported field kind")
    return out

interface JsonDecodeN:
    def from_jsonn(str s) -> Self

@derive(Json, JsonDecodeN)
struct Item:
    int64 id

# A struct whose fields are collections of a nested derived struct, plus a direct
# nested field — every shape that needs the reflected element type.
@derive(Json, JsonDecodeN)
struct Bag:
    list[Item] items
    dict[str, Item] index
    Item lead

def main():
    auto b = Bag([Item(7), Item(9)], {"a": Item(5)}, Item(1))
    # Decode the built-in encoding with the library decoder, inspect by key/index
    # (order-independent) — agreement with the built-in oracle.
    auto rt = Bag.from_jsonn(b.to_json())
    print(rt.items[0].id == 7)
    print(rt.items[1].id == 9)
    print(len(rt.items) == 2)
    print(rt.index["a"].id == 5)
    print(len(rt.index) == 1)
    print(rt.lead.id == 1)

    # Empty collections decode to empty.
    auto e = Bag([], {}, Item(0))
    auto re = Bag.from_jsonn(e.to_json())
    print(len(re.items) == 0)
    print(len(re.index) == 0)
    print(re.lead.id == 0)
