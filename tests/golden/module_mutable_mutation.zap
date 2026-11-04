# A reference-typed mutable global dict: its CONTENTS are mutated across calls
# (a membership-guarded increment), and the binding itself is reassigned to a
# fresh object — both observed later. (It deliberately avoids `dict.get(k, d)`
# on a module-global receiver, which monomorphization does not yet specialize —
# see the separate core report; index + `in` on a module global work.)
dict[str, int64] tally = {}

def record(str k):
    if k in tally:
        tally[k] = tally[k] + 1
    else:
        tally[k] = 1

def reset():
    tally = {}

def main():
    record("a")
    record("a")
    record("b")
    print(tally["a"])  # 2 — contents mutated across calls
    print(tally["b"])  # 1
    print(len(tally))  # 2
    reset()
    print(len(tally))  # 0 — reassigned to a fresh empty dict, persisted
