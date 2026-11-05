# claim: SCOPE-5
# SPEC §12 (I#475, D-0118): `spawn` supports three callable forms — a named
# function, a user-struct method call, and a first-class function value — and
# captures the receiver / arguments / closure eagerly in the spawning coroutine
# (D-0034). A `scope:` joins all three; each deposits one value into a buffered
# channel, drained and summed after the join. The sum is order-independent, so
# the output is deterministic.
#
# Eager capture is proven two ways: the closure `push` captures `bias` at its
# creation (mutating `bias` afterward must not change what the child sees), and
# the method receiver `Adder(100)` is captured at the spawn.
struct Adder:
    int64 base
    def add(self, channel[int64] out, int64 n):
        out <- self.base + n

def send(channel[int64] out, int64 v) -> int64:
    out <- v
    return v

def main():
    auto out = channel[int64](4)
    auto a = Adder(100)
    int64 bias = 7
    # A closure over the locals `out` and `bias` (I#163), bound to a function
    # value and spawned below.
    fn(int64) -> int64 push = lambda n: send(out, n + bias)
    # The closure already captured `bias == 7`; this later write must not leak
    # into the spawned child.
    bias = 999
    scope:
        spawn send(out, 1)  # named function
        spawn a.add(out, 20)  # method: receiver Adder(100) captured
        spawn push(300)  # function value: captures out + bias(==7)
    # Dedent: all three have completed. Drain and sum (order-independent).
    int64 total = 0
    int64 i = 0
    while i < 3:
        total = total + <-out
        i = i + 1
    print(total)
