# Golden fixture: an exception raised in the decorated body propagates out
# through a behavioral wrapper, skipping `after` (D-0007/D-0036; #652).
#
# `risky(5)` runs before -> body -> after -> returns. `risky(-1)` raises inside
# the body: the exception unwinds through the generated wrapper (so `after` never
# runs) and is caught in `main`, proving the wrapper does not swallow it.

decorator guarded:
    def before(ctx):
        print("before", ctx.name)
    def after(ctx, result):
        print("after", result)

@guarded
def risky(int64 x) -> int64:
    if x < 0:
        raise ValueError("negative")
    return x

def main():
    print(risky(5))
    try:
        print(risky(-1))
    except ValueError as e:
        print("caught", e)
