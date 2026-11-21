# Golden fixture (#1237): a spawned child inherits a SNAPSHOT of the parent's
# context at spawn time (Python `copy_context`), and the child's own `set` does
# NOT leak back to the parent after the scope join.
ContextVar[int64] V = ContextVar[int64]("v", default=0)

def child(channel[int64] out):
    out <- V.get()             # inherited snapshot: the parent's 1
    auto tok = V.set(2)        # child mutates its own context
    out <- V.get()             # child sees its own 2
    V.reset(tok)

def main():
    auto out = channel[int64](2)
    auto top = V.set(1)
    scope:
        spawn child(out)
    print(<-out)               # 1 — inherited snapshot
    print(<-out)               # 2 — child's own binding
    print(V.get())             # 1 — child mutation did not leak back
    V.reset(top)
