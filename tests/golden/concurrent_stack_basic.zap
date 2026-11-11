# Single-threaded surface coverage for the lock-free Treiber ConcurrentStack[T]
# (#184, D-0148): comma-ok pop on empty, LIFO order, drain-then-refill.
# Deterministic; identical under both collectors.

def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def main():
    ConcurrentStack[int64] st = new_concurrent_stack()
    auto e0, ok0 = st.pop()
    print(b2i(ok0))  # 0  (empty)
    st.push(1)
    st.push(2)
    st.push(3)
    auto a, oka = st.pop()
    print(a.unwrap())  # 3  (LIFO)
    auto b, okb = st.pop()
    print(b.unwrap())  # 2
    auto c, okc = st.pop()
    print(c.unwrap())  # 1
    auto e1, ok1 = st.pop()
    print(b2i(ok1))  # 0  (drained)
    st.push(9)  # refill after full drain
    auto d, okd = st.pop()
    print(d.unwrap())  # 9
