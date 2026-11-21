# Golden fixture (#1237): a ContextVar bound in a coroutine is visible to a DEEP
# callee that reads it AFTER the coroutine parked and resumed (a channel receive
# forces the suspend). The deep callee never receives the value as a parameter —
# it reads the implicit coroutine-local context, which must survive park/resume.
ContextVar[int64] CUR = ContextVar[int64]("cur", default=-1)

def deep() -> int64:
    # Two frames below the `set`, no parameter threads the value here.
    return CUR.get()

def worker(channel[int64] gate, channel[int64] out):
    auto tok = CUR.set(42)
    auto g0 = <-gate  # park here until main releases us (suspend)
    out <- deep()  # read the binding after the resume
    CUR.reset(tok)

def main():
    auto gate = channel[int64](1)
    auto out = channel[int64](1)
    scope:
        spawn worker(gate, out)
        gate <- 0  # release the worker
        print(<-out)
