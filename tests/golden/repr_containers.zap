# Golden fixture: native list/dict/set rendering through the Display protocol
# (#274, D-0124), Python shapes. Elements/keys/values render in repr mode (so
# strings are quoted); empty forms are Python-exact: [] , {} (empty dict), and
# set() (NOT {}). Containers nest and carry structs/tuples/enums recursively,
# and str()/f-string match print().
struct Point:
    int64 x
    int64 y

def main():
    list[int64] xs = [1, 2, 3]
    print(xs)  # [1, 2, 3]
    list[str] ss = ["a", "b"]
    print(ss)  # ['a', 'b']  string elements quoted
    list[int64] empty = []
    print(empty)  # []
    # A single-entry str dict: a str-keyed dict's iteration (hence repr) order is
    # unspecified and, since #1064 seeded the str hash, varies across processes —
    # so a multi-entry str dict is not order-stable to print. One entry pins the
    # order while still covering the non-empty `{'key': val}` repr shape (the
    # inter-entry comma is covered by the list reprs above). An int-keyed dict
    # would be deterministic (int hashing is unseeded), but this line's point is
    # the quoted str key.
    dict[str, int64] d = {"k": 1}
    print(d)  # {'k': 1}
    dict[str, int64] ed = {}
    print(ed)  # {}  empty dict
    set[int64] s = {7}
    print(s)  # {7}
    set[int64] es = set[int64]()
    print(es)  # set()  empty set is set(), not {}
    list[Point] pts = [Point(x=1, y=2)]
    print(pts)  # [Point(x=1, y=2)]  struct elements render fully
    list[list[int64]] nested = [[1], [2, 3]]
    print(nested)  # [[1], [2, 3]]  nested containers
    print(str(xs))  # [1, 2, 3]  str() matches print
    print(f"{ss}")  # ['a', 'b']  f-string matches print
