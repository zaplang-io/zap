# Golden fixture: user-defined behavioral decorators — `before`/`after` form
# (D-0036, `design/0004-decorators.md`; #652 of epic #651).
#
# `@traced` declares `before(ctx)` and `after(ctx, result)` hooks. Applying it
# generates — at compile time, before sema, visible via `zap expand` — a wrapper
# with `add`'s EXACT signature that runs `before`, calls the original, runs
# `after` (seeing the real return value), and returns it. `ctx.name` is the
# decorated function's name; arguments and the return value thread through.

decorator traced:
    def before(ctx):
        print("enter", ctx.name)
    def after(ctx, result):
        print("exit", ctx.name, result)

@traced
def add(int64 a, int64 b) -> int64:
    print("body", a, b)
    return a + b

def main():
    auto r = add(2, 3)
    print("result", r)
