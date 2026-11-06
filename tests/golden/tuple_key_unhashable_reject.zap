# Compile-fail fixture: a tuple is hashable only when ALL of its elements are
# (CPython parity, #680 / D-0049). A `list` has no stable hash, so a tuple
# containing one cannot be a dict key or set element — rejected at compile time
# with a stable coded diagnostic (ZAP6001), never an internal compiler error.
def main():
    auto d = {(1, [2, 3]): 7}
    print(len(d))
