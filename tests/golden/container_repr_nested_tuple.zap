# Golden fixture: a container whose elements are a variadic `tuple[T]`
# (`Elem::VarTuple`) reprs by recursing into each tuple's `(…)` form (#1516).
# The synthesized `$crepr$N` container renderer derived its name from the memo
# map's length only AFTER building its (recursive) body, so an element whose own
# renderer was synthesized mid-body (a nested container) collided on the same
# `$crepr$N` name and shadowed the outer renderer at link time — the container
# then rendered with the wrong element renderer (the elements printed as their
# raw buffer words). Reserving the memo name BEFORE building the body gives each
# nested renderer a distinct index. No prior top-level `print` of the inner type
# is done here, so the inner renderer is forced to synthesize mid-body and the
# regression is actually exercised (a prior print would pre-cache and mask it).
def main():
    # the headline repro: a list of tuples
    print([tuple([1, 2]), tuple([3, 4])])  # [(1, 2), (3, 4)]

    # a single-element inner tuple keeps its trailing comma inside the container
    print([tuple([9])])  # [(9,)]

    # str-element tuples: element repr quotes the strings
    print([tuple(["x"]), tuple(["y", "z"])])  # [('x',), ('y', 'z')]

    # a dict whose values are tuples (keys snapshot in stable order here: one key)
    print({"a": tuple([1, 2])})  # {'a': (1, 2)}

    # a single-element set of a tuple (deterministic repr; set order is otherwise
    # unspecified, #1066)
    print(set([tuple([1, 2])]))  # {(1, 2)}

    # deeper nesting: a list of lists of tuples — all three renderers synthesize
    # in one nested pass
    print([[tuple([1, 2])], [tuple([3])]])  # [[(1, 2)], [(3,)]]

    # indexing still reads back the precise tuple (unchanged by the fix)
    auto nested = [tuple([5, 6]), tuple([7, 8])]
    print(nested[0])  # (5, 6)
    print(nested[1][1])  # 8
