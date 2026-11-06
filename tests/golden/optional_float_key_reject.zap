# Compile-fail fixture: an `Optional[T]` key is hashable only when `T` is
# (#704). A float has no stable hash, so `Optional[float]` as a dict key is
# rejected with a stable coded diagnostic (ZAP6001) — the same clean reject as
# any other unhashable key, never an internal compiler error.
def main():
    dict[Optional[float], int64] d = {}
    Optional[float] k = Optional.Some(1.5)
    d[k] = 1
    print(len(d))
