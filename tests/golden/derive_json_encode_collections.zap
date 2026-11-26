# E5 (#1551/#1559): library `@derive(Json)` ENCODE for `list[T]` and
# `dict[str, V]` collection fields. Extends the byte-identical encode generator
# (#1554) to the collection surface the built-in gained in #1530 / #1531
# (`gen_json_encode_field` list/dict arms, `crates/ir/src/lower.rs`).
#
# PROOF METHOD DIFFERS BY KIND, on purpose:
#   * `list[T]` is ORDERED, so the library encode is BYTE-IDENTICAL to the
#     built-in: `l.to_json() == l.to_jsonbytes()` (every assertion prints True).
#   * `dict[str, V]` iteration order is UNSPECIFIED and seed-dependent (#1066) —
#     the built-in's own `derive_json_dict_field` golden deliberately avoids any
#     byte / `==` assertion for that reason and proves the codec by keyed
#     inspection of the decoded value. This golden follows the same rule: a
#     multi-entry dict is proven by ROUND-TRIP — parse the library's
#     `to_jsonbytes()` back with the built-in `from_json` and inspect by key
#     (order-independent). An empty and a single-entry dict are order-trivial, so
#     those ARE asserted byte-identical, pinning the object envelope + key
#     escaping + value leaf against the built-in.
#
# The library encode builds every collection text by hand — a `list[str]` of
# element / `"k":v` fragments, `",".join`ed and bracket/brace-wrapped — so list
# ordering and separators match the built-in's `zap_csv_join` + `[]` / `{}`
# layout. Each element/value leaf is produced by the same per-kind dispatch as a
# scalar field (via `f.ty.elem` / `f.ty.value` comptime element-type access), and
# a nested derived struct recurses through its own `to_jsonbytes()`.
#
# Byte-identical (list + small dict) equality is gated by the CI LLVM-18 matrix
# (Docker/LLVM down on the maintainer box; `zap ir` proves lowering only). The
# built-in Json codegen, `DERIVABLE_TRAITS` and the prelude are untouched.

@derivable(JsonBytes)
def _derive_jsonbytes[T](T value) -> str:
    list[str] parts = []
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == "list":
            # A `list[T]` field: `[` + comma-join of per-element leaves + `]`,
            # elements in list order. The element leaf uses the SAME per-kind
            # dispatch as a scalar field, keyed on the comptime element type
            # `f.ty.elem` (#1253).
            auto xs = reflect.field(value, comptime f.index)
            list[str] elems = []
            for x in xs:
                if f.ty.elem.name == "int64":
                    elems.append(json.dumps(JsonValue.Int(x)))
                elif f.ty.elem.name == "str":
                    elems.append(json.dumps(JsonValue.Str(x)))
                elif f.ty.elem.name == "bool":
                    elems.append(json.dumps(JsonValue.Bool(x)))
                elif f.ty.elem.name == "float64":
                    elems.append(json.dumps(JsonValue.Float(x)))
                elif f.ty.elem.kind.name == "struct":
                    elems.append(x.to_jsonbytes())
                else:
                    comptime.error(f.span, "JsonBytes: unsupported list element kind")
            parts.append("\"" + f.name + "\":[" + ",".join(elems) + "]")
        elif f.ty.kind.name == "dict":
            # A `dict[str, V]` field: `{` + comma-join of `"k":v` entries + `}`,
            # keys JSON-escaped. Iteration order is unspecified (#1066); the
            # entries are built in whatever order `.items()` yields, which
            # round-trips regardless (the byte-exact assertions below use only an
            # empty / single-entry dict, where order is trivial).
            auto d = reflect.field(value, comptime f.index)
            list[str] entries = []
            for k, v in d.items():
                str kq = json.dumps(JsonValue.Str(k))
                if f.ty.value.name == "int64":
                    entries.append(kq + ":" + json.dumps(JsonValue.Int(v)))
                elif f.ty.value.name == "str":
                    entries.append(kq + ":" + json.dumps(JsonValue.Str(v)))
                elif f.ty.value.name == "bool":
                    entries.append(kq + ":" + json.dumps(JsonValue.Bool(v)))
                elif f.ty.value.name == "float64":
                    entries.append(kq + ":" + json.dumps(JsonValue.Float(v)))
                elif f.ty.value.kind.name == "struct":
                    entries.append(kq + ":" + v.to_jsonbytes())
                else:
                    comptime.error(f.span, "JsonBytes: unsupported dict value kind")
            parts.append("\"" + f.name + "\":{" + ",".join(entries) + "}")
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

# A nested derived struct used as a list element and a dict value.
@derive(Json, JsonBytes)
struct Item:
    int64 id

# `list[T]` for every element kind the built-in supports.
@derive(Json, JsonBytes)
struct Lists:
    list[int64] nums
    list[str] tags
    list[Item] items

# `dict[str, V]` for every value kind.
@derive(Json, JsonBytes)
struct Dicts:
    dict[str, int64] scores
    dict[str, str] labels
    dict[str, Item] items

def main():
    # --- list[T]: ORDERED, so byte-identical to the built-in encoder.
    auto l = Lists([1, 2, 3], ["a", "x\"y"], [Item(7), Item(9)])
    print(l.to_json() == l.to_jsonbytes())
    # Empty lists byte-identical too (`[]`).
    auto le = Lists([], [], [])
    print(le.to_json() == le.to_jsonbytes())

    # --- dict[str, V]: empty + single-entry are order-trivial, so byte-identical.
    auto d1 = Dicts({"a": 1}, {"k": "v"}, {"i": Item(5)})
    print(d1.to_json() == d1.to_jsonbytes())
    auto de = Dicts({}, {}, {})
    print(de.to_json() == de.to_jsonbytes())

    # --- dict[str, V] multi-entry: order unspecified (#1066), so proven by
    # round-trip — parse the LIBRARY encode back with the built-in decoder and
    # inspect by key (order-independent), mirroring derive_json_dict_field.
    auto m = Dicts({"a": 1, "b": 2}, {"k": "v", "j": "w"}, {"i": Item(5), "h": Item(6)})
    auto rt = Dicts.from_json(m.to_jsonbytes())
    print(rt.scores["a"] == 1)
    print(rt.scores["b"] == 2)
    print(len(rt.scores) == 2)
    print(rt.labels["k"] == "v")
    print(rt.labels["j"] == "w")
    print(rt.items["i"].id == 5)
    print(rt.items["h"].id == 6)
    print(len(rt.items) == 2)
