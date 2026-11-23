# Golden fixture for zaplang-io/zap#1326: an impossible nominal-struct match arm
# in a scalar specialization must be elided — no invalid struct field reads, no
# LLVM codegen panic — while the genuine struct specialization keeps the arm and
# its `as` binding. Regression surfaced after #1310 unblocked native inference.
import datetime

# Destructure a Date through an `as` binding, then feed the binding to a
# field-reading builtin (datetime.combine). With T = Date the arm runs and binds
# `day`; with any scalar T the `Date()` arm is statically impossible, so it is
# dropped and the fallback raises instead of emitting a FieldGet on a scalar.
def promote[T](T value) -> DateTime:
    match value:
        case Date() as day:
            return datetime.combine(day, datetime.time(0, 0, 0))
        case _:
            raise TypeError("expected Date")

# A scalar specialization must compile and route straight to the fallback; the
# generic wrapper proves the dead arm lowered cleanly across several scalar
# kinds (int64/float64/str), catching the TypeError the fallback raises.
def reject[T](T value) -> str:
    try:
        auto promoted = promote(value)
        return datetime.format(promoted)
    except TypeError:
        return "rejected"

def main():
    # Supported specialization: the Date arm matches and binds `day`.
    auto d = datetime.date(2026, 11, 23)
    auto dt = promote(d)
    print(dt.year)  # 2026
    print(dt.month)  # 11
    print(dt.day)  # 23
    print(dt.hour)  # 0

    # Scalar specializations: the impossible Date arm is elided, fallback raises.
    print(reject(1))  # rejected
    print(reject(3.5))  # rejected
    print(reject("x"))  # rejected
