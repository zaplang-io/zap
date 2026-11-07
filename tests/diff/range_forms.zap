# diff: match
# Positive-step `range` forms match CPython (epic #374, walled garden): 1-arg
# `range(n)` is `[0, n)`, 2-arg `range(a, b)` is `[a, b)`, and 3-arg
# `range(a, b, step)` with a positive step counts up. Sums over each range pin
# the element sequence. (Negative/zero step diverge — see range_negative_step.zap
# and #759.)
def main():
    auto a = 0
    for i in range(5):
        a = a + i
    print(a)
    auto b = 0
    for i in range(2, 6):
        b = b + i
    print(b)
    auto c = 0
    for i in range(1, 10, 2):
        c = c + i
    print(c)
