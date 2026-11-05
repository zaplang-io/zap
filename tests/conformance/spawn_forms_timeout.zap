# claim: SCOPE-6
# SPEC §12 (I#475 + D-0094): the method and function-value `spawn` forms compose
# with `scope(timeout=…)`. On the deadline the scope cancels its children —
# running their `finally` cleanup — and raises `ScopeTimeout` to the caller, whose
# report names every outstanding coroutine by its identity: a method as
# `Struct.method`, a function value by its callee name. The complete pending set
# is reported in spawn order (D-0094).
#
# Both children block on `<-ch` (the parent drains `ready` first to guarantee it),
# so the deadline finds both parked. Cleanup is observed through a buffered
# channel, drained afterward and counted — order-independent, so deterministic.
struct Server:
    def serve(self, channel[int64] ready, channel[int64] ch, channel[int64] cleaned):
        try:
            ready <- 1
            auto v = <-ch
            print("wrong receive", v)
        finally:
            cleaned <- 1

def waiter(channel[int64] ready, channel[int64] ch, channel[int64] cleaned) -> int64:
    try:
        ready <- 1
        auto v = <-ch
        print("wrong receive", v)
    finally:
        cleaned <- 1
    return 0

def main():
    auto ready = channel[int64]()
    auto ch = channel[int64]()
    auto cleaned = channel[int64](2)
    auto s = Server()
    # A bare top-level function taken as a first-class value (I#163).
    fn(channel[int64], channel[int64], channel[int64]) -> int64 w = waiter
    try:
        scope(timeout=1000):
            spawn s.serve(ready, ch, cleaned)  # method
            spawn w(ready, ch, cleaned)  # function value
            auto a = <-ready
            auto b = <-ready
    except ScopeTimeout as e:
        print(e)
    int64 cleanups = 0
    int64 i = 0
    while i < 2:
        cleanups = cleanups + <-cleaned
        i = i + 1
    print("cleanups", cleanups)
