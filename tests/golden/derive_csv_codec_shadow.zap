# E4 (#1429/#1480): shadow-prove that the built-in `@derive(Csv)` codec can be
# expressed as plain library `@derivable` generators, WITHOUT replacing the
# built-in. This is the second E4 *codec* slice, after Json (#1470), and the one
# that proves the mechanism reaches a codec with NO source-accessible DOM or
# module behind it: unlike Json (`JsonValue` + public `json.dumps`/`json.loads`),
# CSV is built entirely in compiler-private codegen (`zap_csv_join` /
# `zap_csv_split` / `zap_csv_quote` / `zap_bool_to_str` / `zap_str_to_i64` …), so
# the shadow must express encode/decode in PURE ZAP SOURCE with string primitives
# only — `str()`, `.split`, `",".join`, `int()` / `float()`, and a manual bool
# parse. It still leans on the two E4 enablers: `reflect.construct` /
# `reflect.set_field` (#1454) and derived-method `Self`-return typing (#1455).
#
# A struct derives BOTH the compiler built-in `Csv` (used through `to_csv` /
# `from_csv`) and two library codec traits, `CsvEncode` (an instance
# `to_csvcodec(self) -> str`) and `CsvDecode` (an associated
# `from_csvcodec(str) -> Self`). The two express the same flat-record codec and
# agree with the built-in by ROUND-TRIP, cross-checked in BOTH directions — the
# agreed proof.
#
# Built-in Csv codegen (`gen_csv_to_method` / `gen_csv_from_method`), the
# `DERIVABLE_TRAITS` registry and the prelude are all untouched. Covers int64 /
# str / bool / float64 (every field kind a flat CSV record supports). v1 str
# values carry no embedded comma/quote/newline — RFC-4180 quoting/escaping is a
# documented follow-up; decode is scalar-only regardless (the #1454 zero-set).

# --- CsvEncode: render each field to its column text and join with commas.
# `str()` renders int64 / float64 and — matching the built-in's Python csv
# convention — bool as `True` / `False`; a `str` field is already its own column.
@derivable(CsvEncode)
def _derive_csvencode[T](T value) -> str:
    list[str] parts = []
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == "int64":
            parts.append(str(reflect.field(value, comptime f.index)))
        elif f.ty.name == "str":
            parts.append(reflect.field(value, comptime f.index))
        elif f.ty.name == "bool":
            parts.append(str(reflect.field(value, comptime f.index)))
        elif f.ty.name == "float64":
            parts.append(str(reflect.field(value, comptime f.index)))
        else:
            comptime.error(f.span, "CsvEncode: unsupported field kind")
    return ",".join(parts)

interface CsvEncode:
    def to_csvcodec(self) -> str

# A manual bool parser — the built-in `to_csv` renders `True` / `False`, and its
# `from_csv` also accepts lowercase `true`; the shadow mirrors both so either
# encoder's output decodes. Anything else is false (lenient, like the built-in).
def _csv_bool(str col) -> bool:
    return col == "True" or col == "true"

# --- CsvDecode: split the record into columns, zero-construct a T, then write
# each field. `reflect.construct` zero-initializes only scalar / str / bytes
# fields (#1454) — a flat CSV record is exactly that, so every supported field
# kind round-trips; nested structs / collections / enums are rejected by the
# built-in `@derive(Csv)` itself, so there is no wider surface to cover here.
@derivable(CsvDecode)
def _derive_csvdecode[T](str s) -> T:
    auto out = reflect.construct(T)
    auto cols = s.split(",")
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == "int64":
            reflect.set_field(out, comptime f.index, int(cols[comptime f.index]))
        elif f.ty.name == "str":
            reflect.set_field(out, comptime f.index, cols[comptime f.index])
        elif f.ty.name == "bool":
            reflect.set_field(out, comptime f.index, _csv_bool(cols[comptime f.index]))
        elif f.ty.name == "float64":
            reflect.set_field(out, comptime f.index, float(cols[comptime f.index]))
        else:
            comptime.error(f.span, "CsvDecode: unsupported field kind")
    return out

interface CsvDecode:
    def from_csvcodec(str s) -> Self

# All four flat-record field kinds; derives the built-in `Csv` (the oracle) plus
# both library codec traits, plus `Eq` so the round-trips compare with `==`.
@derive(Eq, Csv, CsvEncode, CsvDecode)
struct Row:
    str city
    int64 pop
    bool capital
    float64 lat

def main():
    auto r = Row(city="Paris", pop=42, capital=True, lat=48.5)

    # library encode -> library decode round-trips to the original value...
    print(Row.from_csvcodec(r.to_csvcodec()) == r)
    # ...the library decode agrees with the built-in encoder...
    print(Row.from_csvcodec(r.to_csv()) == r)
    # ...and the built-in decode agrees with the library encoder.
    print(Row.from_csv(r.to_csvcodec()) == r)

    # A second row exercises False / a negative int / a different float, so the
    # bool and numeric columns are not pinned to one value.
    auto r2 = Row(city="Berlin", pop=-7, capital=False, lat=0.0)
    print(Row.from_csvcodec(r2.to_csvcodec()) == r2)
    print(Row.from_csvcodec(r2.to_csv()) == r2)
    print(Row.from_csv(r2.to_csvcodec()) == r2)
