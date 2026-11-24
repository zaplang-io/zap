# Regression for #1362: a bare payload value `T` assigned into an existing
# `Optional[T]` local (or module-level global) via a plain `name = value`
# reassignment must box as a tagged/boxed `Some(T)`, never store the raw
# scalar/pointer into the Optional slot. The plain `Stmt::Assign` to a `Name`
# lowered its right-hand side with a bare `self.expr(value)`, bypassing the
# `expr_coerced` chokepoint every other accepted Optional position uses (#1262,
# #1075/#1080/#1195), so a later `is_some()` / `unwrap()` read the raw bits as an
# Optional object and SIGSEGV'd (the issue's `0xffffffffffffd5d0` = the bit
# pattern of `-10800`). This is the reassignment partner of the #1262
# constructor/field/index wrap. All reads below are native
# (is_some/is_none/unwrap/unwrap_or) so the tagged-vs-raw storage is actually
# observed — it only matters once the native collector runs.

struct Item:
    int64 value


# A module-level mutable global (D-0112): the `Stmt::Assign` to a `Name` path
# covers a global's slot identically, so it must box the same way.
Optional[int64] g_offset = None


def describe(Optional[int64] opt) -> int64:
    # The reassigned local crosses an ordinary call boundary: the callee receives
    # the Optional by value and must see a well-formed `Some`/`None`.
    if opt.is_some():
        return opt.unwrap()
    return -1


def set_global(int64 v):
    # A bare payload reassigned into the module global.
    g_offset = v


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

    # INT_MIN, the negative extreme.
    Optional[int64] lo = None
    lo = -9223372036854775808
    print(lo.unwrap())  # -9223372036854775808

    # Reassign Some -> None -> Some again; each store is a well-formed box.
    Optional[int64] cycle = Optional.Some(1)
    print(cycle.unwrap())  # 1
    cycle = None
    print(cycle.is_none())  # True
    cycle = 2
    print(cycle.is_some())  # True
    print(cycle.unwrap())  # 2

    # Cross a call with the reassigned local.
    Optional[int64] crossed = None
    crossed = 77
    print(describe(crossed))  # 77
    crossed = None
    print(describe(crossed))  # -1

    # An already-Optional source copies through unchanged — never double-wrapped
    # into Some(Some(..)).
    Optional[int64] src = Optional.Some(6)
    Optional[int64] dst = None
    dst = src
    print(dst.unwrap())  # 6

    # Reference (pointer) payload: a bare `str` reassigned into `Optional[str]`.
    # A raw pointer store would be mis-read; the empty string must still read as
    # Some (not None).
    Optional[str] name = None
    name = "hi"
    print(name.is_some())  # True
    print(name.unwrap())  # hi
    name = ""
    print(name.is_some())  # True
    print(name.unwrap() == "")  # True
    name = None
    print(name.is_none())  # True

    # Reference struct payload.
    Optional[Item] slot = None
    slot = Item(42)
    print(slot.is_some())  # True
    print(slot.unwrap().value)  # 42

    # A collection payload (`list[int64]`): a bare list reassigned into
    # `Optional[list[int64]]` must box.
    Optional[list[int64]] nums = None
    nums = [10, 20, 30]
    print(nums.is_some())  # True
    print(nums.unwrap()[1])  # 20

    # unwrap_or on the empty side after a reassignment back to None.
    Optional[int64] bk = Optional.Some(5)
    bk = None
    print(bk.unwrap_or(-9))  # -9

    # The module-level global, reassigned through a bare payload in a callee.
    print(g_offset.is_none())  # True
    set_global(-10800)
    print(g_offset.is_some())  # True
    print(g_offset.unwrap())  # -10800
