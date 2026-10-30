# Golden fixture: a generator *method on a data-carrying enum* (I#449). This is
# the enum analogue of `generator_method.zap` (I#429): a `self`-bearing `def`
# whose body contains `yield` lowers to the same iterator state machine as a
# free-function generator (I#67) — the only difference from the struct case is
# that `self` is the tier-3 (tagged-struct) enum receiver rather than a struct.
# The generator `match`es `self` to read the captured payload (`Hold.start`)
# once, then yields `start, start+1, …` across each suspend/resume.
# `IntBox.Hold(10).nums(5)` yields 10,11,12,13,14; summing them gives 60.
enum IntBox:
    Hold(int64 start)

    def nums(self, int64 n):
        match self:
            case IntBox.Hold(start):
                auto i = 0
                while i < n:
                    yield start + i
                    i = i + 1


def main():
    auto c = IntBox.Hold(10)
    auto total = 0
    for x in c.nums(5):
        total = total + x
    print(total)
