# Golden fixture: a generator *method* (I#429). A `self`-bearing `def` whose body
# contains `yield` lowers to the same iterator state machine as a free-function
# generator (I#67) — the only difference is that `self` is captured into the
# state frame like any other parameter. `Seq(start=10).nums(5)` yields
# 10,11,12,13,14 (reading the captured `self.start` across each suspend/resume);
# summing them gives 60.
struct Seq:
    int64 start

    def nums(self, int64 n):
        auto i = 0
        while i < n:
            yield self.start + i
            i = i + 1


def main():
    auto c = Seq(start=10)
    auto total = 0
    for x in c.nums(5):
        total = total + x
    print(total)
