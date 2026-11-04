# A reference-typed mutable global: its CONTENTS are mutated across calls, and
# the binding itself can be reassigned to a fresh object — both observed later.
dict[str, int64] tally = {}

def record(str k):
    tally[k] = tally.get(k, 0) + 1

def reset():
    tally = {}

def main():
    record("a")
    record("a")
    record("b")
    print(tally.get("a", 0))  # 2 — contents mutated across calls
    print(tally.get("b", 0))  # 1
    print(len(tally))  # 2
    reset()
    print(len(tally))  # 0 — reassigned to a fresh empty dict, persisted
