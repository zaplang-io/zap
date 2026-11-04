# Golden fixture: a `wrap(call)` decorator stacked under a `before`/`after`
# decorator, applied bottom-up (D-0036; #653).
#
# `@trace` (before/after) over `@twice` (wrap) over `step`: the decorator
# nearest the declaration (`twice`) wraps first, so the chain is
# `step -> step$deco1 -> step$orig`. `twice` calls the original twice (the first
# result is discarded, the second returned). `trace` then sees that second
# result as `after`'s `result`. Hooks fire trace.before, (twice body), then
# trace.after — the specified, deterministic order.

decorator trace:
    def before(ctx):
        print("enter", ctx.name)
    def after(ctx, result):
        print("exit", ctx.name, result)

decorator twice:
    def wrap(call):
        def inner(*args):
            call(*args)
            return call(*args)
        return inner

@trace
@twice
def step(int64 x) -> int64:
    print("body", x)
    return x + 1

def main():
    print("final", step(10))
