# Golden fixture (#1237): a ContextVar carries values of a scalar (int64), a
# reference struct, and a GC-heap str — exercising that a reference value stored
# in the coroutine context is correctly ROOTED (traced + kept alive) while a
# scalar word is stored without being mis-scanned as a pointer. A GC is forced
# between the `set` and the `get` to prove the stored references survive.
# (Nested-generic value types like list[int64] are a documented follow-up —
# the shared `chan_elem_from_expr` path resolves only simple/named element
# types today, as AtomicArray[T] and channel[T] do.)
struct Span:
    int64 id

ContextVar[int64] A = ContextVar[int64]("a", default=0)
ContextVar[Span] B = ContextVar[Span]("b")
ContextVar[str] C = ContextVar[str]("c", default="none")

def churn():
    # Allocate garbage to encourage a collection while the context holds live
    # references (Span, str) — they must not be reclaimed.
    int64 i = 0
    while i < 10000:
        list[int64] tmp = [i, i + 1, i + 2]
        i = i + 1

def main():
    auto ta = A.set(7)
    auto tb = B.set(Span(id=99))
    auto tc = C.set("hello")
    churn()
    print(A.get())  # 7
    print(B.get().id)  # 99
    print(C.get())  # hello
    A.reset(ta)
    B.reset(tb)
    C.reset(tc)
