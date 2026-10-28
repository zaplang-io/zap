# Defaults are evaluated per call (D-0038): a mutable default like `[]` is a
# fresh value each call, so it never accumulates across calls the way Python's
# evaluate-once-at-def default does. An explicitly passed list keeps reference
# semantics (D-0023), so the callee's append is visible to the caller.

def app(int64 x, list[int64] acc = []) -> int64:
    acc.append(x)
    return len(acc)

def main():
    print(app(1))            # 1  (fresh [])
    print(app(2))            # 1  (fresh [] again — not 2)
    print(app(3))            # 1  (still fresh)
    auto shared = [10, 20]
    print(app(99, shared))   # 3  (explicit list appended)
    print(len(shared))       # 3  (caller's list was mutated — reference type)
