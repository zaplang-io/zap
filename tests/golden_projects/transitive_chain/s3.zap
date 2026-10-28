# Mirrors a per-service package depending on core (D-0011 §8).
import core

def put(int64 n) -> int64:
    return core.sign(n) + 10
