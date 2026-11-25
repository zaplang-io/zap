# diff: match
# tuple[T] element-wise == / != matches CPython's tuple equality (#1474): equal
# length + equal elements, by value (str elements compared by content). Only
# tuple(iterable) is used (the Zap-only typed/empty forms are in the golden).
def main():
    print(tuple([1, 2, 3]) == tuple([1, 2, 3]))
    print(tuple([1, 2, 3]) == tuple([1, 2]))
    print(tuple([1, 2, 3]) == tuple([1, 2, 4]))
    print(tuple([1, 2, 3]) != tuple([1, 2, 3]))
    print(tuple([1, 2, 3]) != tuple([1, 2]))
    print(tuple(["x", "y"]) == tuple(["x", "y"]))
    print(tuple(["x", "y"]) == tuple(["x", "z"]))
    print(tuple([]) == tuple([]))
