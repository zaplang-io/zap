# Golden fixture (#1237): two coroutines interleaved on a SINGLE worker M
# (ZAP_MAXPROCS=1, see the .env sidecar) never observe or overwrite each other's
# binding. A ready/go barrier forces BOTH to `set` before EITHER reads back, so a
# shared (non-isolated) context would make both read the last writer's value
# (sum 40); isolation yields each coroutine its own id (sum 30).
ContextVar[int64] W = ContextVar[int64]("w", default=-1)

def worker(int64 id, channel[int64] ready, channel[int64] go, channel[int64] out):
    auto tok = W.set(id)
    ready <- 1  # signal "I have set my binding"
    auto g = <-go  # wait until BOTH have set (forces interleave)
    out <- W.get()  # read back MY own id
    W.reset(tok)

def main():
    auto ready = channel[int64](2)
    auto go = channel[int64](2)
    auto out = channel[int64](2)
    int64 r1 = 0
    int64 r2 = 0
    scope:
        spawn worker(10, ready, go, out)
        spawn worker(20, ready, go, out)
        auto a = <-ready  # both have set before we release either
        auto b = <-ready
        go <- 0
        go <- 0
        r1 = <-out
        r2 = <-out
    print(r1 + r2)  # 30 iff isolated (10 + 20)
