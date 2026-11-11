# Single-threaded surface coverage for the lock-free ConcurrentMap[K, V] (#184,
# D-0147): put (insert + update-shadow), get (comma-ok), contains, delete
# (tombstone), get_or, and the approximate length. Deterministic; identical
# under both collectors; no heap-byte assertions.

def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def main():
    ConcurrentMap[str, int64] m = new_concurrent_map()
    print(m.length())  # 0
    m.put("a", 1)
    m.put("b", 2)
    m.put("c", 3)
    print(m.length())  # 3
    auto va, oka = m.get("a")
    print(va.unwrap())  # 1
    print(b2i(oka))  # 1
    print(b2i(m.contains("b")))  # 1
    print(b2i(m.contains("z")))  # 0
    m.put("a", 100)  # update an existing key (shadows the old node)
    auto va2, ok2 = m.get("a")
    print(va2.unwrap())  # 100
    print(m.length())  # 3  (update, not a new key)
    print(b2i(m.delete("b")))  # 1  (was present)
    print(b2i(m.contains("b")))  # 0
    print(m.length())  # 2
    print(b2i(m.delete("b")))  # 0  (already gone)
    print(m.get_or("b", 7))  # 7  (absent -> default)
    print(m.get_or("c", 7))  # 3  (present)
    auto vz, okz = m.get("z")
    print(b2i(okz))  # 0
