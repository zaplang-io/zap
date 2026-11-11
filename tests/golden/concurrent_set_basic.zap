# Single-threaded surface coverage for ConcurrentSet[K] (#184, D-0148), a
# membership wrapper over ConcurrentMap: add (idempotent), contains, remove,
# approximate length. Deterministic; identical under both collectors.

def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def main():
    ConcurrentSet[int64] s = new_concurrent_set()
    print(s.length())  # 0
    s.add(10)
    s.add(20)
    s.add(10)  # idempotent
    print(s.length())  # 2
    print(b2i(s.contains(10)))  # 1
    print(b2i(s.contains(30)))  # 0
    print(b2i(s.remove(10)))  # 1
    print(b2i(s.contains(10)))  # 0
    print(s.length())  # 1
    print(b2i(s.remove(10)))  # 0  (already gone)
