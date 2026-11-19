# Golden fixture: struct elements over a typed channel (#1100).
#
# The issue reproducer. A value received from `channel[Entry]` must come back as
# an `Entry`, not an opaque pointer, so field access on it compiles and reads the
# delivered struct. (Before the fix the channel element was classified from a
# bare name and collapsed to a pointer, so the received value read back as `str`
# and `entry.amount` failed to lower with "field access on a non-struct value".)
# Covers both the buffered path and the unbuffered rendezvous; `main` does all
# the receiving and printing, so the output is deterministic regardless of how
# the producer coroutine is scheduled.
struct Entry:
    int amount

def produce(channel[Entry] ch, int n):
    ch <- Entry(n)

def main():
    # Buffered: send then receive without a second coroutine.
    auto inbox = channel[Entry](1)
    inbox <- Entry(42)
    auto entry = <-inbox
    print(entry.amount)  # 42

    # Unbuffered: the send in `produce` rendezvouses with the receive here, so
    # the struct crosses a real blocking handoff and the field is still read.
    auto rendezvous = channel[Entry]()
    spawn produce(rendezvous, 7)
    auto got = <-rendezvous
    print(got.amount)  # 7
