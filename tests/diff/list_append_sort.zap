# diff: match
# list append + in-place sort match CPython (epic #374, walled garden). append
# grows the list; sort() orders ascending and sort(reverse=True) descending
# (stable, in place, #336); str lists sort lexicographically. Elements are
# printed by iteration so the order after each sort is deterministic (these are
# the only two list methods — append, sort). CPython is the oracle.
def main():
    list[int64] xs = [3, 1, 2]
    xs.append(5)
    xs.append(0)
    print(len(xs))
    xs.sort()
    for x in xs:
        print(x)
    xs.sort(reverse=True)
    for x in xs:
        print(x)
    list[str] ys = ["bb", "a", "ccc"]
    ys.sort()
    for y in ys:
        print(y)
