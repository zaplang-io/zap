# Golden fixture: the `select` statement (D-0031, #1002).
#
# Each select below has exactly one ready case (or none, falling to `default`),
# so the chosen case — and thus the output — is deterministic regardless of the
# uniform-random tie-break among ready cases. Covered: `default` fall-through, a
# ready send case, a ready receive case, a comma-ok receive on a closed channel
# (ok == False), and a blocking receive woken by a spawned coroutine's send
# (the park/wake path, no `default`).
def sender(channel[int64] e):
    e <- 55

def main():
    auto ch = channel[int64](2)  # buffered (cap 2), empty

    # Nothing ready -> `default`.
    select:
        case v = <-ch:
            print(100 + v)
        default:
            print(1)

    # Send ready (buffer has space): the only ready case.
    select:
        case ch <- 42:
            print(2)
        default:
            print(999)

    # Receive ready (buffer now holds 42).
    select:
        case v = <-ch:
            print(v)
        default:
            print(999)

    # Comma-ok receive on a closed channel: ready, ok == False.
    auto d = channel[int64]()
    close(d)
    select:
        case v, ok = <-d:
            if ok:
                print(777)
            else:
                print(3)

    # Blocking receive woken by a spawned sender (no `default`): parks until
    # `sender` rendezvouses, then binds the delivered value.
    auto e = channel[int64]()
    spawn sender(e)
    select:
        case v = <-e:
            print(v)
