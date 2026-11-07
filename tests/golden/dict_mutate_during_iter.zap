# Golden (epic #374, D-0130): `for k in d` iterates a snapshot of the keys taken
# at loop entry, so deleting keys during the loop is safe and the loop still sees
# every key that was present at entry — no CPython-style "changed size during
# iteration" RuntimeError. Observed order-independently: the loop body runs once
# per original key, and the dict ends empty.
def main():
    dict[int64, int64] d = {1: 10, 2: 20, 3: 30}
    int64 seen = 0
    for k in d:
        seen = seen + 1
        del d[k]
    print(seen)
    print(len(d))
