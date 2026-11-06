# Golden fixture: bool collection ELEMENTS render True/False (#274, D-0124).
# A collection stores bool as the i64 element word, but the IR keeps its bool
# identity (Elem::Bool, since bool is Ty::I1, distinct from every integer), so
# the Display protocol renders a bool list/set element and dict value as
# True/False rather than the 1/0 integer form. (A `char` element renders
# Python-style too, via the sibling Elem::Char since #709; see
# char_collections.zap.)
def main():
    list[bool] flags = [True, False, True]
    print(flags)  # [True, False, True]
    set[bool] on = {True}
    print(on)  # {True}
    # A single-entry dict keeps the golden independent of Zap's dict iteration
    # order (hash order, not insertion order), while still exercising a bool
    # dict value rendering as True/False.
    dict[str, bool] d = {"on": True}
    print(d)  # {'on': True}
    print(str(flags))  # [True, False, True]  str() matches
    # A plain int list is unaffected (renders integers).
    list[int64] ns = [1, 0]
    print(ns)  # [1, 0]
