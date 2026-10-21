# Golden (I#175): single-thread mechanics of the shared sync primitives through
# the Python-shaped threading surface — RLock reentrancy, Semaphore permit
# counting with a timed acquire, Event set/clear/is_set/timed-wait, and the
# Condition acquire/notify/release path with no waiter. Deterministic outcomes
# (the two timed waits elapse their short timeout and report False); no
# heap-byte assertions.
def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def main() -> int64:
    auto r = threading.RLock()
    r.acquire()
    r.acquire()
    print(b2i(r.try_acquire()))
    r.release()
    r.release()
    r.release()

    auto s = threading.Semaphore(2)
    s.acquire()
    s.acquire()
    print(b2i(s.acquire_timeout(0.01)))
    s.release()
    print(b2i(s.acquire_timeout(0.01)))

    auto e = threading.Event()
    print(b2i(e.is_set()))
    e.set_()
    print(b2i(e.is_set()))
    e.wait()
    print(b2i(e.wait_timeout(0.01)))
    e.clear()
    print(b2i(e.is_set()))
    print(b2i(e.wait_timeout(0.01)))

    auto c = threading.Condition()
    c.acquire()
    c.notify(1)
    c.notify_all()
    c.release()

    print(999)
    return 0
