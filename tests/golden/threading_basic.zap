# Golden (I#175): the Python-threading-shaped OS-thread compat surface, smoke
# test. A `threading.Thread(target=...)` runs its target on its OWN dedicated OS
# thread (beyond MAXPROCS, pinned via the #172 LockOSThread substrate) while
# still participating in precise-GC stop-the-world + safepoints and rooting
# through the shadow stack; `join()` blocks the main coroutine until the thread
# finishes, so the output order is deterministic. NOT recommended for new code
# (idiomatic Zap = coroutines + channels); this is a migration escape hatch.
def work():
    print("hi from the thread")

def main() -> int64:
    auto t = threading.Thread(target=work)
    t.start()
    t.join()
    print("joined")
    return 0
