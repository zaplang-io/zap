# Golden fixture: a `value struct` with NO __str__/__repr__/@derive(Repr) gets the
# same automatic, total, field-wise default render as a reference struct —
# `TypeName(field=value, ...)`, never an address — across print(), str(), and
# f-strings (the capability model's auto/total/overridable Repr, D-0139/#810,
# realized by the Display protocol #274/D-0124). Value structs render exactly like
# reference structs; the `value` modifier changes storage, not rendering.
value struct V:
    int64 a
    int64 b


def main():
    auto v = V(a=3, b=4)
    print(v)  # V(a=3, b=4)
    print(str(v))  # V(a=3, b=4)
    print(f"v={v}")  # v=V(a=3, b=4)
