# #1603 + #1608: `reflect.construct` widenings that complete the DECODE side of a
# recursive library codec.
#   * #1603 — `reflect.construct(T)` zero-inits an ENUM-typed field to the enum's
#     first variant (an enum has no natural zero; the decoder overwrites it).
#   * #1608 — `reflect.construct` of a CONTAINER type builds a TYPED empty
#     collection (`list[E]()` / `dict[K,V]()` / `set[E]()`), so `auto acc =
#     reflect.construct(E)` pins the element type and a recursive decoder can
#     append into it. With #1604 (reflected type args) this lets a library codec
#     DECODE nested collections of arbitrary depth, round-tripping with encode.
#
# Gated by the CI LLVM-18 matrix (`zap ir` proves lowering locally).

def _rd_int(JsonValue v) -> int64:
    match v:
        case JsonValue.Int(i):
            return i
        case _:
            return 0

# Recursive element encoder (list nesting, scalar leaf).
def _enc[E](E x) -> str:
    if reflect.type_info(E).name == "int64":
        return json.dumps(JsonValue.Int(x))
    elif reflect.type_info(E).kind.name == "list":
        list[str] parts = []
        for y in x:
            parts.append(_enc[reflect.type_info(E).elem](y))
        return "[" + ",".join(parts) + "]"
    else:
        comptime.error("unsupported")

# Recursive element decoder: a scalar leaf, or a `list` built into a TYPED empty
# container (#1608) that recursion appends into.
def _dec[E](JsonValue v) -> E:
    if reflect.type_info(E).name == "int64":
        match v:
            case JsonValue.Int(i):
                return i
            case _:
                return 0
    elif reflect.type_info(E).kind.name == "list":
        auto acc = reflect.construct(E)
        match v:
            case JsonValue.Array(xs):
                for jv in xs:
                    acc.append(_dec[reflect.type_info(E).elem](jv))
            case _:
                pass
        return acc
    else:
        comptime.error("unsupported")

def _arr(dict[str, JsonValue] m, str k) -> JsonValue:
    if k in m:
        return m[k]
    return JsonValue.Array([])

@derivable(NEnc)
def _derive_nenc[T](T value) -> str:
    list[str] parts = []
    comptime for f in reflect.type_info(T).fields:
        parts.append(
            "\"" + f.name + "\":" + _enc[f.ty](reflect.field(value, comptime f.index)),
        )
    return "{" + ",".join(parts) + "}"

@derivable(NDec)
def _derive_ndec[T](str s) -> T:
    auto out = reflect.construct(T)
    dict[str, JsonValue] m = {}
    match json.loads(s):
        case JsonValue.Object(fields):
            m = fields
        case _:
            m = {}
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == "list":
            reflect.set_field(out, comptime f.index, _dec[f.ty](_arr(m, f.name)))
        elif f.ty.name == "int64":
            reflect.set_field(out, comptime f.index, _rd_int(m[f.name]))
        elif f.ty.kind.name == "enum":
            # #1603: the enum field was zero-initialized by `reflect.construct`
            # (its first variant); a full enum decode is a codec concern, not this
            # reflect.construct proof — leave the constructed base.
            pass
        else:
            comptime.error("unsupported field")
    return out

interface NEnc:
    def nenc(self) -> str

interface NDec:
    def ndec(str s) -> Self

# #1608: nested-collection round-trip.
@derive(NEnc, NDec)
struct Grid:
    list[list[int64]] g

# #1603: a struct with an enum field must `reflect.construct` (zero-init the enum
# field to its first variant) and decode the rest.
enum Tag:
    A
    B
    C

@derive(NDec)
struct Holder:
    int64 n
    Tag t

def main():
    # Nested-list round-trip: decode(encode(x)) recovers the structure.
    auto src = Grid([[1, 2], [3]])
    auto rt = Grid.ndec(src.nenc())
    print(rt.g[0][1] == 2)
    print(rt.g[1][0] == 3)
    print(len(rt.g) == 2)
    print(len(rt.g[0]) == 2)

    # A struct with an enum field decodes (the enum field zero-inits, #1603).
    auto h = Holder.ndec("{\"n\":5}")
    print(h.n == 5)
    match h.t:
        case Tag.A:
            print(True)
        case _:
            print(False)
