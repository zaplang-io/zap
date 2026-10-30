# Golden fixture / bisection (I#451, axis = struct-yield): a generator method
# that YIELDS A STRUCT, in a single file. This is the control
# `generator_method_paginator.zap` with exactly one change: `pages` yields a
# `Page` struct instead of a bare int64. Isolates the "yield a struct" axis of
# #451 from the "imported module" axis (that one is the sibling project fixture
# `golden_projects/gen_method_module_struct`). Same shape/counts as the control:
# `Pager(total=10, page_size=3)` yields page-start tokens 0, 3, 6, 9 — 4 pages,
# the last starting at 9.
struct Page:
    int64 start

struct Pager:
    int64 total
    int64 page_size

    def next_token(self, int64 t) -> int64:
        return t + self.page_size

    def pages(self, int64 token):
        auto t = token
        while t < self.total:
            yield Page(start=t)
            t = self.next_token(t)


def main():
    auto p = Pager(total=10, page_size=3)
    auto count = 0
    auto last = 0
    for pg in p.pages(0):
        count = count + 1
        last = pg.start
    print(count)
    print(last)
