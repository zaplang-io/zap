# Golden fixture: behavioral decorators stack **bottom-up** (D-0036; #652).
#
# `@outer` over `@inner` over `def compute` applies like Python's
# `outer(inner(compute))`: the decorator nearest the declaration (`inner`) wraps
# the body first. At run time the hooks fire outer.before, inner.before, body,
# inner.after, outer.after — the specified, deterministic order.

decorator outer:
    def before(ctx):
        print("outer before")
    def after(ctx, result):
        print("outer after", result)

decorator inner:
    def before(ctx):
        print("inner before")
    def after(ctx, result):
        print("inner after", result)

@outer
@inner
def compute(int64 x) -> int64:
    print("compute body")
    return x * 2

def main():
    print("final", compute(5))
