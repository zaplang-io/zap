# Cross-module (#1262): construct an imported struct whose `Optional[T]` fields
# take bare payloads in the *importing* module. Before the fix the payload was
# stored unboxed (`iconst` straight into the Optional field), so a native read
# (`unwrap`/`is_some`) dereferenced the integer as an Optional object and
# segfaulted. Covers a scalar payload, the adversarial zero, a reference (`str`)
# payload, and omitted fields that keep their `None` default — all read natively.
import shapes


def main():
    auto p = shapes.Point(x=1, y=0, label="origin")
    print(p.x.unwrap())  # 1
    print(p.y.is_some())  # True  — a wrapped 0, not a null Optional
    print(p.y.unwrap())  # 0
    print(p.label.unwrap())  # origin

    # Omitted fields keep their declared `None` defaults in the importer too.
    auto q = shapes.Point(y=5)
    print(q.x.is_none())  # True
    print(q.y.unwrap())  # 5
    print(q.label.is_none())  # True
