# Golden fixture: socket read deadline (ticket #485, D-0096).
#
# A server coroutine binds an ephemeral port, accepts one connection, sets a
# short read timeout via `net.set_read_timeout`, and then reads. The client
# connects but deliberately sends NOTHING, so the server's `recv` parks on the
# netpoller and the deadline elapses: `recv` raises `TimeoutError` (distinct from
# a clean EOF, which is an empty result), which the server catches inside `try`
# and turns into a deterministic marker. The marker — not any wall-clock number —
# is what the golden asserts, so it does not race under either collector or arch.
#
# The handshake is deterministic: the client holds the connection open (sending
# nothing) until the server signals, over the `done` channel, that its read has
# already timed out. Only then does the client close and the program exit.

def server(channel[int64] ports, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    ports <- net.port(l)
    auto c = net.accept(l)
    net.set_read_timeout(c, 50)
    try:
        auto msg = net.recv(c, 1024)
        print("unexpected: read returned")
    except TimeoutError:
        print("read-timeout-fired")
    net.close_conn(c)
    net.close_listener(l)
    done <- 1

def main():
    auto ports = channel[int64]()
    auto done = channel[int64]()
    spawn server(ports, done)
    auto port = <-ports
    auto c = net.connect("127.0.0.1", port)
    # Send nothing; block until the server reports its read timed out.
    auto ack = <-done
    net.close_conn(c)
    print("done")
