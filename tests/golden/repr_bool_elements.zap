# Golden fixture: bool collection ELEMENTS render True/False (#274, D-0124).
# A collection stores bool as the i64 element word, but the IR keeps its bool
# identity (Elem::Bool, since bool is Ty::I1, distinct from every integer), so
# the Display protocol renders a bool list/set element and dict value as
# True/False rather than the 1/0 integer form. (A `char` element still renders
# as its codepoint integer: char shares Ty::I32 with int32 in the IR — a
# documented pre-existing limitation, tracked separately.)
def main():
    list[bool] flags = [True, False, True]
    print(flags)  # [True, False, True]
    set[bool] on = {True}
    print(on)  # {True}
    dict[str, bool] d = {"a": True, "b": False}
    print(d)  # {'a': True, 'b': False}
    print(str(flags))  # [True, False, True]  str() matches
    # A plain int list is unaffected (renders integers).
    list[int64] ns = [1, 0]
    print(ns)  # [1, 0]
