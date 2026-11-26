# Golden fixture: list.sort / sorted are catchable (#1598). A scratch-allocation
# failure inside `zap_list_sort_by` now raises a catchable `MemoryError` instead
# of aborting, so both public entry points carry an unwind edge inside a `try`.
#
# A healthy host never fails the scratch malloc, so this fixture cannot drive the
# allocation-failure leg itself (that is exercised by the host ZIR invoke tests
# in crates/ir/src/lib.rs and, end to end, only by an LLVM build with injected
# faults). What it DOES lock is the in-repo-observable half of the fix: a sort
# placed inside `try/except MemoryError` still runs normally, the handler is not
# taken on success, the result is correct, and — the acceptance criterion's
# success side — the same list sorts again to the same order (re-sortable, never
# left in a half-moved state). Intent documentation / regression wall.

def ident(int64 x) -> int64:
    return x

def join_ints(list[int64] xs) -> str:
    auto out = ""
    auto first = True
    for x in xs:
        if first:
            out = out + str(x)
            first = False
        else:
            out = out + "," + str(x)
    return out

def main():
    # The exact reproduction from the issue: an in-place sort with a key inside a
    # MemoryError handler. On a healthy host it just sorts; the except clause is
    # never entered.
    list[int64] xs = [3, 2, 1]
    auto caught = False
    try:
        xs.sort(key=ident)
    except MemoryError:
        caught = True
    print(join_ints(xs))  # 1,2,3
    print(caught)  # False

    # Re-sortable: sorting the already-sorted list again is a no-op in value and
    # leaves it well-formed (the failure path would have freed its scratch and
    # left the input untouched, so a retry is always safe).
    try:
        xs.sort()
    except MemoryError:
        caught = True
    print(join_ints(xs))  # 1,2,3

    # sorted(...) inside a try takes the same catchable path and returns a new,
    # correctly ordered list while leaving its input alone.
    list[int64] src = [5, 4, 3, 2, 1]
    list[int64] ys = []
    try:
        ys = sorted(src)
    except MemoryError:
        caught = True
    print(join_ints(ys))  # 1,2,3,4,5
    print(join_ints(src))  # 5,4,3,2,1

    # Empty and singleton inputs stay on the same path (n < 2 returns before any
    # scratch allocation) and never raise.
    list[int64] none = []
    try:
        none.sort()
        print(len(sorted(none)))  # 0
    except MemoryError:
        caught = True
    list[int64] one = [7]
    try:
        one.sort()
    except MemoryError:
        caught = True
    print(join_ints(one))  # 7

    print(caught)  # False
    print("sort_catchable_1598: PASS")
