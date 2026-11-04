# Golden fixture: behavioral decorators with arguments (D-0036; #652).
#
# `decorator tag(str label)` / `decorator level(int64 n)` take their own
# arguments; `@tag("payments")` and `@level(2)` wire those values into the
# generated code (bound to synthetic locals the hooks reference). Combined with
# bottom-up stacking, `tag.before` runs first and `level.after` sees the return.

decorator tag(str label):
    def before(ctx):
        print("tag", label, ctx.name)

decorator level(int64 n):
    def after(ctx, result):
        print("level", n, result)

@tag("payments")
@level(2)
def charge(int64 amount) -> int64:
    return amount + 1

def main():
    print("done", charge(99))
