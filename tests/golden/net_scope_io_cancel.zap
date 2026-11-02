# Cancellation at/before the I/O park, without a timer or a peer release. The
# failing sibling waits until the I/O child is inside its try/finally region.

def fail_after(channel[int64] entered):
    auto ready = <-entered
    raise ValueError("cancel I/O fixture")

def reader(
    bytes socket,
    channel[int64] entered,
    Atomic[int64] cancelled,
    Atomic[int64] cleaned,
):
    try:
        entered <- 1
        net.recv(socket, 1)
    except Cancelled:
        cancelled.add(1)
    finally:
        net.close_conn(socket)
        cleaned.add(1)

def peer(bytes listener, channel[int64] done):
    auto socket = net.accept(listener)
    net.close_listener(listener)
    try:
        auto eof = net.recv(socket, 1)
        if len(eof) != 0:
            raise ValueError("expected cancelled reader EOF")
    finally:
        net.close_conn(socket)
    done <- 1

def main():
    Atomic[int64] cancelled = Atomic(0)
    Atomic[int64] cleaned = Atomic(0)
    for i in range(0, 32):
        auto listener = net.listen("127.0.0.1", 0)
        auto done = channel[int64]()
        spawn peer(listener, done)
        auto socket = net.connect("127.0.0.1", net.port(listener))
        auto entered = channel[int64]()
        auto failed = False
        try:
            scope:
                spawn reader(socket, entered, cancelled, cleaned)
                spawn fail_after(entered)
        except Exception:
            failed = True
        if not failed or cancelled.load() != i + 1 or cleaned.load() != i + 1:
            raise ValueError("cancelled I/O did not clean up before scope exit")
        auto complete = <-done
        net.close_conn(socket)
    print("cancelled TCP reads cleaned before scope exit")
    print("descriptor reuse: 32")
