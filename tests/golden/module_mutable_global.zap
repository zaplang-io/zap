# Module-level MUTABLE state (D-0112, #662 Phase 2): persistent, reassignable
# bindings at module scope — scalars and reference (heap) types. They are read,
# reassigned, and mutated across functions, and persist across calls (the light
# Go/Python concurrency model of epic #661: no `global` keyword, a bare
# assignment to a module binding reassigns it in place).
int64 counter = 0
list[int64] log = []

def bump() -> int64:
    counter += 1
    log.append(counter)
    return counter

def main():
    print(counter)  # 0  — initial value
    bump()
    bump()
    print(counter)  # 2  — `+=` across two calls persisted
    counter = 100
    print(counter)  # 100 — whole reassignment persisted
    print(len(log))  # 2  — container grew across calls
