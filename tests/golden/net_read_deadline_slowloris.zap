# Golden fixture: the whole-operation read deadline (ticket #567) bounds a
# slow-dribble (slowloris) request over PLAINTEXT, through `http.read_request`.
# The per-park timeout (`net.set_read_timeout`, #485) alone cannot stop this: a
# peer that sends one byte periodically, each well UNDER the per-park timeout,
# keeps resetting the per-park clock forever and the request never completes.
# `net.set_read_deadline` adds a budget for the ENTIRE request regardless of byte
# pacing, so the read is cut off with a catchable `TimeoutError`.
#
# The server sets a GENEROUS per-park timeout (so the 50 ms dribbles never trip
# it — proving the whole-op deadline, not the per-park one, is what fires) and a
# SHORT whole-op deadline, then calls `http.read_request`. The client dribbles an
# incomplete request (bytes that never terminate the headers) one at a time and
# keeps dribbling until a send fails — which only happens once the server's
# whole-op deadline has fired and it has closed the conn. If the whole-op
# deadline were broken the server would read forever (the dribbles keep the
# per-park clock alive) and the client would dribble forever, so a regression
# HANGS rather than silently passing. The marker — not any wall-clock number — is
# what the golden asserts, so it does not race under either collector or arch.


def slow_server(channel[int64] portch, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    net.set_read_timeout(c, 10000)  # generous per-park: a 50 ms dribble never trips it
    net.set_read_deadline(c, 300)  # whole-op budget: bounds the entire request
    try:
        auto req = http.read_request(c)
        print("unexpected: read returned")
    except TimeoutError:
        print("net-slowloris-fired")
    net.close_conn(c)
    net.close_listener(l)
    done <- 1


def main():
    auto portch = channel[int64]()
    auto done = channel[int64]()
    spawn slow_server(portch, done)
    auto port = <-portch
    auto c = net.connect("127.0.0.1", port)
    # Dribble an incomplete request forever, one byte at a time under the per-park
    # timeout; stop only when a send fails (the server closed after its whole-op
    # deadline fired).
    while True:
        try:
            net.send(c, "x")
        except Exception:
            break
        time.sleep_ns(time.millis(50))
    auto ack = <-done
    net.close_conn(c)
    print("done")
