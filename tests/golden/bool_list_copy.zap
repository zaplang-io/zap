# Golden fixture: iterating a `list[bool]` yields `bool` loop values that append
# back into a `list[bool]`, preserving both values including `False` (#1241).
#
# A `bool` element is stored and physically read as an `i64` word (the
# `Elem::Bool` tag keeps its identity only for the Display protocol, #274), but
# the value a program OBSERVES when it iterates or indexes the list is a `bool`,
# not an integer. Before the fix the loop variable / indexed read came back
# typed `i64`, so `list[bool].append(value)` saw a mismatched element kind
# (`Elem::I64` vs `Elem::Bool`) and raised `error[ZAP6001]: append() value has
# the wrong element type` — even though the value round-trips byte-for-byte, and
# even though the identical iterate+append pattern on `list[int64]`,
# `list[float64]`, and `list[str]` worked. The read-back now narrows to the
# logical scalar type (`bool`), so it re-appends and re-stores cleanly, matching
# how string iteration already yields a `char`.
#
# Both element values — `True` AND `False` — must survive the copy, and a `bool`
# read back and printed must render `True`/`False` (not its `-1` all-ones int
# widening, cf. print_scalar_values.zap).

def copy_all[T](list[T] src) -> list[T]:
    list[T] out = []
    for value in src:
        out.append(value)
    return out

enum Attr:
    Flags(list[bool] values)
    Empty

def snapshot(Attr a) -> list[bool]:
    list[bool] out = []
    match a:
        case Attr.Flags(vs):
            for value in vs:
                out.append(value)
        case Attr.Empty:
            pass
    return out

def main():
    # --- the exact issue repro: statement-`for` iterate + append ---
    list[bool] source = [True, False]
    list[bool] copied = []
    for value in source:
        copied.append(value)
    print(len(copied))  # 2
    print(copied)  # [True, False]
    print(copied[0])  # True
    print(copied[1])  # False   (False is preserved, not dropped or flipped)
    assert len(copied) == 2
    assert copied[0] == True
    assert copied[1] == False

    # --- the indexed-read sibling: `append(source[i])` ---
    list[bool] via_index = []
    via_index.append(source[1])
    via_index.append(source[0])
    print(via_index)  # [False, True]
    assert via_index[0] == False
    assert via_index[1] == True

    # --- the generic `[T]` helper over bool and the kinds that already worked:
    # the loop-variable type must round-trip for every scalar element kind ---
    list[bool] gb = copy_all([True, False, True])
    print(gb)  # [True, False, True]
    assert len(gb) == 3
    list[int64] gi = copy_all([1, 2, 3])
    print(gi)  # [1, 2, 3]
    list[str] gs = copy_all(["a", "b"])
    print(gs)  # ['a', 'b']
    list[float64] gf = copy_all([1.5, 2.5])
    print(gf)  # [1.5, 2.5]

    # --- a list[bool] extracted from an enum variant (the issue's enum case) ---
    list[bool] from_enum = snapshot(Attr.Flags([True, False, True]))
    print(from_enum)  # [True, False, True]
    assert len(from_enum) == 3
    assert from_enum[1] == False
    print(snapshot(Attr.Empty))  # []
