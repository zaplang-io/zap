# claim: SCOPE-6
# SPEC §12 (I#475 + D-0094): the method and function-value `spawn` forms compose
# with `scope(timeout=…)`. Both children block on a channel; the deadline cancels
# them and the scope raises `ScopeTimeout` to the caller, whose report names every
# outstanding coroutine by its identity — a method as `Struct.method`, a function
# value by its callee name — in spawn order (the complete pending set, D-0094).
#
# The parent drains `ready` twice first, so the deadline is guaranteed to find
# both children parked on `<-ch`; the report is therefore deterministic under
# `MAXPROCS=1`. (That `finally`/`with` cleanup runs under cancellation is covered
# form-independently by `scope_timeout` / `scope_cleanup`, since cleanup lives in
# the child body regardless of how it was spawned.)
struct Server:
    def serve(self, channel[int64] ready, channel[int64] ch):
        ready <- 1
        auto v = <-ch
        print("wrong receive", v)

def waiter(channel[int64] ready, channel[int64] ch) -> int64:
    ready <- 1
    auto v = <-ch
    print("wrong receive", v)
    return 0

def main():
    auto ready = channel[int64]()
    auto ch = channel[int64]()
    auto s = Server()
    # A bare top-level function taken as a first-class value (I#163).
    fn(channel[int64], channel[int64]) -> int64 w = waiter
    try:
        scope(timeout=1000):
            spawn s.serve(ready, ch)  # method
            spawn w(ready, ch)  # function value
            auto a = <-ready
            auto b = <-ready
    except ScopeTimeout as e:
        print(e)
