# Consumer: construct the `Pager` with a top-level function as its `bump`
# closure, then iterate its generator method. `bump` advances the page start by
# 3 each step, so `Pager(total=10, page_size=3)` yields page starts 0, 3, 6, 9 —
# 4 pages, the last starting at 9. Observed before the #453 fix: empty stdout.
import pager

def plus3(int64 x) -> int64:
    return x + 3

def main():
    fn(int64) -> int64 b = plus3
    auto p = pager.Pager(total=10, page_size=3, bump=b)
    auto count = 0
    auto last = 0
    for pg in p.pages(0):
        count = count + 1
        last = pg.start
    print(count)
    print(last)
