# #1604: a derive generator can recurse into a container's ELEMENT type through
# a generic helper. Two comptime capabilities this proves, both over a plain
# generic helper (not just the top-level `@derivable` body):
#   1. `reflect.type_info(E)` resolves a type parameter `E` bound to a CONTAINER
#      (`list[int64]`), so a kind dispatch `if reflect.type_info(E).kind.name ==
#      "list"` folds and prunes in the helper (needs the full-type substitution,
#      not a name-only one).
#   2. `reflect.type_info(E).elem` resolves as an explicit TYPE ARGUMENT, so the
#      helper can recurse at the element type: `_enc_elem[reflect.type_info(E).elem](y)`.
#
# Together these let a library codec handle NESTED collections (`list[list[T]]`,
# `dict[str, list[T]]`, `list[dict[str, T]]`) of arbitrary depth — the built-in
# `@derive(Json)` nested-collection surface (#1533) — which a non-recursive
# generator cannot. This golden exercises the ENCODE direction (ordered, so the
# output bytes are deterministic). Gated by the CI LLVM-18 matrix.

# A recursive element encoder: a scalar leaf, or a `list` / `dict` that recurses
# into its element / value type named via `reflect.type_info(E).elem` / `.value`.
def _enc_elem[E](E x) -> str:
    if reflect.type_info(E).name == "int64":
        return json.dumps(JsonValue.Int(x))
    elif reflect.type_info(E).kind.name == "list":
        list[str] parts = []
        for y in x:
            parts.append(_enc_elem[reflect.type_info(E).elem](y))
        return "[" + ",".join(parts) + "]"
    elif reflect.type_info(E).kind.name == "dict":
        list[str] entries = []
        for k, v in x.items():
            entries.append(
                json.dumps(JsonValue.Str(k)) + ":" + _enc_elem[reflect.type_info(E).value](v),
            )
        return "{" + ",".join(entries) + "}"
    else:
        comptime.error("unsupported element type")

@derivable(NestEnc)
def _derive_nestenc[T](T value) -> str:
    list[str] parts = []
    comptime for f in reflect.type_info(T).fields:
        parts.append(
            "\"" + f.name + "\":" + _enc_elem[f.ty](reflect.field(value, comptime f.index)),
        )
    return "{" + ",".join(parts) + "}"

interface NestEnc:
    def nestenc(self) -> str

@derive(NestEnc)
struct N:
    list[list[int64]] grid
    dict[str, list[int64]] groups
    list[dict[str, int64]] rows

def main():
    auto n = N([[1, 2], [3]], {"a": [4, 5]}, [{"x": 7}])
    # Nested arrays/objects recurse to arbitrary depth, in list order.
    print(
        n.nestenc() == "{\"grid\":[[1,2],[3]],\"groups\":{\"a\":[4,5]},\"rows\":[{\"x\":7}]}",
    )
    # A deeper nesting: list[list[list[int64]]] via a struct field.
    auto d = Deep([[[1], [2, 3]], [[4]]])
    print(d.nestenc() == "{\"cube\":[[[1],[2,3]],[[4]]]}")

@derive(NestEnc)
struct Deep:
    list[list[list[int64]]] cube
