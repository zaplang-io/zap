# E5 (#1551/#1554): BYTE-IDENTICAL library `@derive(Json)` ENCODE. This is the
# first E5 *replace* slice. Unlike the E4 Json shadow (#1470), which proved only
# ROUND-TRIP agreement through an UNORDERED `dict[str, JsonValue]` DOM + a final
# `json.dumps`, this golden proves the library encoder produces the EXACT SAME
# BYTES as the built-in `@derive(Json)` encoder (`gen_json_to_method` /
# `gen_json_encode_field` in `crates/ir/src/lower.rs`).
#
# The byte-identical trick is to build the object envelope BY HAND in field
# order — a `list[str]` of `"name":<leaf>` fragments, `",".join`ed and
# brace-wrapped — exactly mirroring the built-in's `zap_csv_join` + `{}` layout
# (no spaces, declaration order, an `Optional` `None` field omits its key
# entirely, D-0065). Each leaf is produced by `json.dumps` of a single
# `JsonValue` leaf, reusing the runtime JSON formatting the DOM path already
# uses, so scalar/string formatting is self-consistent with the built-in.
#
# The proof is a RUNTIME STRING EQUALITY `s.to_json() == s.to_jsonbytes()` for
# each value, checked under the CI LLVM-18 matrix (Docker/LLVM is down on the
# maintainer box; `zap ir` proves only lowering). Every assertion prints `True`.
#
# Built-in Json codegen, the `DERIVABLE_TRAITS` registry and the prelude are all
# untouched — this slice adds only the library `JsonBytes` generator. Covers
# int64 / str (incl. escape-bearing) / bool / float64, a nested derived struct,
# and an `Optional[int64]` field (present AND absent). `list` / `dict`
# collection fields are the next E5 slice (#1559).

# --- JsonBytes: build `{"f":<leaf>,...}` in declaration order, byte-identical to
# the built-in. A nested derived struct recurses through its own `to_jsonbytes()`
# (byte-identical to its own `to_json()` by the same assertion below, so the
# nesting is byte-identical by induction). An `Optional` field emits its fragment
# only when `Some`; a `None` omits the key (matching the built-in's D-0065 rule).
@derivable(JsonBytes)
def _derive_jsonbytes[T](T value) -> str:
    list[str] parts = []
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == "struct":
            parts.append(
                "\"" + f.name + "\":" + reflect.field(value, comptime f.index).to_jsonbytes(),
            )
        elif f.ty.kind.name == "optional":
            # The E4 shadow exercises `Optional[int64]`; the bound local annotates
            # the matched value's instance (I#154). `Some(x)` -> emit the fragment;
            # `None` -> omit the key (no fragment pushed), byte-identical to the
            # built-in, which pushes no `"name":value` list entry for a `None`.
            Optional[int64] ov = reflect.field(value, comptime f.index)
            match ov:
                case Optional.Some(x):
                    parts.append("\"" + f.name + "\":" + json.dumps(JsonValue.Int(x)))
                case Optional.None:
                    pass
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

# A nested derived struct: built-in `Json` plus the library `JsonBytes` trait, so
# the outer encoder can recurse through the nested `to_jsonbytes()`.
@derive(Json, JsonBytes)
struct Inner:
    int64 a
    str s

# All four scalar/str field kinds.
@derive(Json, JsonBytes)
struct Flat:
    int64 x
    str s
    bool b
    float64 f

# A nested derived struct field and an `Optional[int64]` scalar.
@derive(Json, JsonBytes)
struct Nested:
    int64 n
    Inner inner
    Optional[int64] opt

# A struct whose `str` fields require JSON escaping, so the library leaf escaping
# (`json.dumps(JsonValue.Str(...))`) is pinned byte-for-byte against the built-in
# `zap_json_escape`: a double quote, a backslash, a newline, a tab, and a
# non-ASCII code point.
@derive(Json, JsonBytes)
struct Escapes:
    str quote
    str backslash
    str newline
    str tab
    str unicode

def main():
    # --- Flat: every scalar/str kind, byte-identical to the built-in encoder.
    auto p = Flat(7, "hi", True, 2.5)
    print(p.to_json() == p.to_jsonbytes())
    # A second Flat pins False / a negative int / a zero float / an empty string.
    auto p2 = Flat(-3, "", False, 0.0)
    print(p2.to_json() == p2.to_jsonbytes())

    # --- Nested: a nested derived struct field recurses byte-identically...
    auto o = Nested(3, Inner(9, "z"), Optional.Some(5))
    print(o.to_json() == o.to_jsonbytes())
    # ...and a `None` optional omits its key byte-identically (D-0065).
    auto e = Nested(1, Inner(0, ""), None)
    print(e.to_json() == e.to_jsonbytes())

    # --- Escapes: string escaping is byte-identical to `zap_json_escape`.
    auto esc = Escapes("a\"b", "c\\d", "e\nf", "g\th", "π")
    print(esc.to_json() == esc.to_jsonbytes())
