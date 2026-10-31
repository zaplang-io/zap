# Repro module (I#453): the passing #451 `gen_method_module_struct` shape plus a
# single FUNCTION-TYPED FIELD on the generator's struct. `self` (carrying the
# `bump` closure) is spilled across the `yield` in `pages`. The closure is NOT
# called inside the generator frame — it is reached through the plain sibling
# method `next_start` — so this isolates "a function value living in the spilled
# generator `self`" as the trigger. This is the exact shape of the AWS SDK
# paginator engine (a `self`-bearing `yield` method whose struct holds the
# caller-supplied fetch closure), zaplang-io/aws PR #22.
struct Page:
    int64 start

struct Pager:
    int64 total
    int64 page_size
    fn(int64) -> int64 bump

    def next_start(self, int64 t) -> int64:
        fn(int64) -> int64 bump = self.bump
        return bump(t)

    def pages(self, int64 token):
        auto t = token
        while t < self.total:
            yield Page(start=t)
            t = self.next_start(t)
