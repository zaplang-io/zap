# Single-threaded surface coverage for the `concurrent` module (ticket #184,
# D-0078): every Counter op and the Queue's comma-ok contract, including the
# drain-then-refill case (a queue that goes empty must still deliver later
# pushes), a reference (str) payload, and FIFO order. Deterministic; identical
# under both collectors.
import concurrent

def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def main():
    # Counter: default start 0, add returns the new value, incr adds 1, and a
    # non-zero start is honored.
    Counter c = concurrent.Counter()
    print(c.get())  # 0
    print(c.add(5))  # 5
    print(c.add(-2))  # 3
    print(c.incr())  # 4
    print(c.get())  # 4
    Counter c2 = concurrent.Counter(100)
    print(c2.get())  # 100

    # Queue[int64]: comma-ok pop on empty, FIFO order, then drain-and-refill.
    Queue[int64] q = new_queue()
    auto e0, ok0 = q.pop()
    print(b2i(ok0))  # 0  (empty)
    q.push(10)
    q.push(20)
    q.push(30)
    auto a, oka = q.pop()
    print(a.unwrap())  # 10
    print(b2i(oka))  # 1
    auto b, okb = q.pop()
    print(b.unwrap())  # 20
    auto c3, okc = q.pop()
    print(c3.unwrap())  # 30
    auto e1, ok1 = q.pop()
    print(b2i(ok1))  # 0  (drained)
    q.push(40)  # refill after full drain
    q.push(50)
    auto d, okd = q.pop()
    print(d.unwrap())  # 40  (must NOT be lost)
    auto f, okf = q.pop()
    print(f.unwrap())  # 50

    # Queue[str]: a reference payload (pointer slot, traced by the GC).
    Queue[str] qs = new_queue[str]()
    qs.push("alpha")
    qs.push("beta")
    auto s1, s1ok = qs.pop()
    print(s1.unwrap())  # alpha
    auto s2, s2ok = qs.pop()
    print(s2.unwrap())  # beta
