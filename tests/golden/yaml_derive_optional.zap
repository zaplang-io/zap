# Golden fixture: `@derive(Yaml)` round-trips a top-level `Optional[T]` field
# (#1514). Regression for the miscompile where an `Optional` field lowered to a
# call to a non-existent `Optional$T__to_yaml` — an unresolved external symbol
# that only the LLVM build caught, because `gen_yaml_to_method` recursed into the
# `Optional` aggregate as if it were a nested `@derive(Yaml)` struct.
#
# The fix gives Yaml parity with `@derive(Json)` (I#165 / D-0065): `to_yaml` emits
# `name: <inner>` when `Some` and `name: null` when `None`; `from_yaml` — which
# shares the JSON `from_json_value` decode core — maps an absent key or an
# explicit `null` back to `None`, so the field round-trips. Covers a scalar
# payload and a nested `@derive(Yaml)` struct payload, both `Some` and `None`.
@derive(Yaml)
struct Addr:
    str city
    int64 zip

@derive(Yaml)
struct Rec:
    str name
    Optional[int64] age
    Optional[Addr] addr

def main():
    # Some scalar + Some nested struct: the scalar encodes inline, the struct as
    # an indented block.
    auto r = Rec(
        name="ada",
        age=Optional.Some(42),
        addr=Optional.Some(Addr(city="paris", zip=75001)),
    )
    print(r.to_yaml())
    auto v = Rec.from_yaml(r.to_yaml())
    print(v.name)  # ada
    print(v.age.unwrap())  # 42
    print(v.addr.unwrap().city)  # paris
    print(v.addr.unwrap().zip)  # 75001

    # None scalar + None nested struct: both emit `null`, decode back to `None`.
    auto r2 = Rec(name="bob", age=None, addr=None)
    print(r2.to_yaml())
    auto v2 = Rec.from_yaml(r2.to_yaml())
    print(v2.name)  # bob
    print(v2.age.is_none())  # True
    print(v2.addr.is_none())  # True
