# Dependency `datekit` (zaplang-io/zap#1326): a generic that destructures a Date
# through an `as` binding inside a match arm, then reads the binding's fields via
# a field-reading builtin. When a consumer in another package instantiates it
# with a scalar, the `Date()` arm is statically impossible and must be elided —
# the regression (surfaced in zaplang-io/datetime-utils#22) was native codegen
# emitting a struct FieldGet on the scalar and panicking LLVM.
import datetime

def promote[T](T value) -> DateTime:
    match value:
        case Date() as day:
            return datetime.combine(day, datetime.time(0, 0, 0))
        case _:
            raise TypeError("expected Date")

def reject[T](T value) -> str:
    try:
        auto promoted = promote(value)
        return datetime.format(promoted)
    except TypeError:
        return "rejected"
