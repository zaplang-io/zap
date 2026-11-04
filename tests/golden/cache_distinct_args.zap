# @cache keys on the WHOLE typed argument tuple (#666): distinct argument
# tuples get distinct cache entries, and the key is order-sensitive, so
# `combine(1, 2)` and `combine(2, 1)` are different entries each computed once,
# while a repeated `combine(1, 2)` is a hit. Deterministic observable-effect
# golden (the `compute` lines), single coroutine.
@cache
def combine(int64 a, int64 b) -> int64:
    print("compute", a, b)
    return a * 10 + b

def main():
    print(combine(1, 2))
    print(combine(1, 2))
    print(combine(2, 1))
    print(combine(1, 2))
