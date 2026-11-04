# @cache stacks with another decorator bottom-up (#666). Written `@traced` over
# `@cache`, this is Python's `traced(cache(work))`: `@cache` (nearest the def)
# wraps the body, and `@traced` wraps the cache, so the trace fires on EVERY
# call — including a cache hit — while the body's `work` print fires only on a
# miss. Deterministic observable-effect golden.
decorator traced:
    def before(ctx):
        print("call", ctx.name)

@traced
@cache
def work(int64 n) -> int64:
    print("work", n)
    return n + 1

def main():
    print(work(5))
    print(work(5))
