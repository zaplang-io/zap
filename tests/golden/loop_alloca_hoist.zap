# Ticket #91: a local declared inside a long-running loop must not re-`alloca`
# a fresh stack slot on every iteration. Before the fix each loop-body `alloca`
# was emitted in place (a *dynamic* alloca, outside the entry block), so the
# stack pointer advanced once per iteration and never unwound until the call
# returned — millions of iterations overflowed the stack and crashed.
#
# With fixed-size allocas hoisted to the function entry block, the slots for
# `x` and `y` are reserved once per call and reused across iterations, so this
# loop runs to completion. The per-iteration *values* are unchanged (each slot
# is re-`store`d every iteration), so the running total is exact.
#
# GC-independent: reproduces identically under Boehm and the precise Immix GC.
def main():
    auto total = 0
    for i in range(0, 3000000):
        auto x = i
        auto y = x + 1
        total = total + (y - x)
    print(total)
