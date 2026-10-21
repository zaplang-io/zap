# Golden (I#175): a daemon thread must NOT keep the program alive. main starts a
# daemon Thread that sleeps forever and never join()s it, then returns. A daemon
# thread's dedicated OS thread is excluded from every program-exit join barrier
# (ded_joinable=0 and not registered in g_live), so the process exits promptly
# and prints only "main done" — if daemon exclusion were broken this would hang
# until the 60s harness timeout. Deterministic output; no heap-byte assertions.
import time

def spin():
    while True:
        time.sleep(0.05)

def main() -> int64:
    auto t = threading.Thread(target=spin, daemon=True)
    t.start()
    print("main done")
    return 0
