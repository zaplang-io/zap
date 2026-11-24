# Regression for #1362: a bare payload value `T` assigned into an existing
# `Optional[T]` local via a plain `name = value` reassignment must box as a
# tagged/boxed `Some(T)`, never store the raw scalar/pointer into the Optional
# slot. The plain `Stmt::Assign` to a `Name` lowered its right-hand side with a
# bare `self.expr(value)`, bypassing the `expr_coerced` chokepoint every other
# accepted Optional position uses (#1262, #1075/#1080/#1195), so a later
# `is_some()` / `unwrap()` read the raw bits as an Optional object and SIGSEGV'd
# (the issue's `0xffffffffffffd5d0` = the bit pattern of `-10800`). This is the
# reassignment partner of the #1262 constructor/field/index wrap. All reads
# below are native (is_some/is_none/unwrap) so the tagged-vs-raw storage is
# actually observed — it only matters once the native collector runs.
#
# Scope: these mirror, via a plain *local reassignment*, exactly the payload
# representations the #1262 `optional_scalar_wrap` golden already proves at
# runtime through the field/index assignment forms — a scalar (incl. the
# tag-vs-None-distinguishing zero), a `str` (pointer/niche) payload, and a
# reference struct. The broader reassignment edge cases (`Some -> None -> Some`
# cycles, crossing a call boundary, an already-`Optional` source, `INT_MIN`) are
# pinned at the IR level by `crates/ir/tests/optional_reassign_lower.rs`.

struct Item:
    int64 value


def main():
    # The issue's exact minimal repro: a negative payload.
    int64 seconds = -10800
    Optional[int64] offset = None
    print(offset.is_none())  # True
    offset = seconds
    print(offset.is_some())  # True
    print(offset.is_none())  # False
    print(offset.unwrap())  # -10800

    # Zero payload: a raw 0 is indistinguishable from an empty Optional, so the
    # discriminant tag is the only thing that keeps is_some() correct.
    Optional[int64] zero = None
    zero = 0
    print(zero.is_some())  # True
    print(zero.unwrap())  # 0

    # Reference (pointer) payload: a bare `str` reassigned into `Optional[str]`.
    Optional[str] name = None
    name = "hi"
    print(name.is_some())  # True
    print(name.unwrap())  # hi

    # Reference struct payload.
    Optional[Item] slot = None
    slot = Item(42)
    print(slot.is_some())  # True
    print(slot.unwrap().value)  # 42
