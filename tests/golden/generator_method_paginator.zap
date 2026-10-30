# Golden fixture: a paginator-shaped generator method (I#429) — the exact shape
# the generated AWS SDK clients emit (per-service paginators are `self`-bearing
# methods that `yield` pages until a continuation token is exhausted). `pages`
# loops, calls a sibling method (`self.next_token`) across each `yield`, and
# stops when the token runs past `self.total`. `Pager(total=10, page_size=3)`
# yields page-start tokens 0, 3, 6, 9 — 4 pages, the last starting at 9.
struct Pager:
    int64 total
    int64 page_size

    def next_token(self, int64 t) -> int64:
        return t + self.page_size

    def pages(self, int64 token):
        auto t = token
        while t < self.total:
            yield t
            t = self.next_token(t)


def main():
    auto p = Pager(total=10, page_size=3)
    auto count = 0
    auto last = 0
    for start in p.pages(0):
        count = count + 1
        last = start
    print(count)
    print(last)
