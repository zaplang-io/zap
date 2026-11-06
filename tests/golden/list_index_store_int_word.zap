# Golden fixture: indexed STORES into collection elements whose kind is in the
# integer-word family — `int`, `bool` (`Elem::Bool`) and `char` (`Elem::Char`).
# Those three kinds are one physical cell (an `i64` word, the `i64` setter
# suffix, an `i64` read-back) and are distinct `Elem` kinds only so the Display
# protocol renders them differently (#274/#709).
#
# Regression guard for #727: a value READ BACK from such a collection has the
# bare `i64` read-back type, so storing it into another `bool`/`char`/`int`
# collection — `want[i] = flags[j]` — was rejected by an exact-`Elem`-identity
# check in `check_index_value` with the self-contradictory
# `indexed assignment expects `i64`, found `i64``. Introduced by #708's
# `Elem::Bool` tag, widened to `char` by #712's `Elem::Char`. Every index form
# (literal, range-derived loop var, arithmetic, and an index that is itself an
# indexed load) and every int-word element type must store and read back here,
# so an IR-type-identity slip on the store path can't silently return.
def main():
    # --- the exact #727 issue repro: list[bool], i64 (auto) index ---
    list[bool] flags = []
    for i in range(4):
        flags.append(False)
    auto depth = 0
    flags[depth] = True
    print(flags)  # [True, False, False, False]

    # --- the jwt-style shape: store a value read back from a list[bool] into a
    # list[bool] element, with an arithmetic index (`depth - 1`). ---
    list[bool] objects = []
    list[bool] want_key = []
    for i in range(4):
        objects.append(False)
        want_key.append(False)
    objects[3] = True
    auto d = 3
    want_key[d - 1] = objects[d]  # bool read-back (i64 word) -> list[bool]
    print(objects)  # [False, False, False, True]
    print(want_key)  # [False, False, True, False]

    # --- int list: all four index forms ---
    list[int] xs = []
    for i in range(5):
        xs.append(i)  # [0, 1, 2, 3, 4]
    xs[0] = 10  # integer-literal index
    for i in range(5):
        xs[i] = xs[i] + 1  # range-derived loop-var index + i64 read
    list[int] pos = [4, 2]
    xs[pos[0]] = xs[pos[1]]  # index is itself an indexed load: a[b[i]]
    print(xs)  # [11, 2, 3, 4, 3]

    # --- char list: store a char read back from the same list (#712 feature
    # must not regress — char element still renders quoted). ---
    list[char] cs = []
    for c in "abc":
        cs.append(c)
    cs[0] = cs[2]  # char read-back (i64 word) -> list[char]
    print(cs)  # ['c', 'b', 'c']
