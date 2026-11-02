def peer(bytes listener, channel[int64] entered, channel[int64] done):
    auto socket = net.accept(listener)
    net.close_listener(listener)
    try:
        net.recv(socket, 1)
        entered <- 1
        while len(net.recv(socket, 4096)) > 0:
            checkpoint()
    finally:
        net.close_conn(socket)
    done <- 1

def trigger(channel[int64] entered):
    auto begun = <-entered
    raise ValueError("cancel handshake fixture")

def dial(int64 port, Atomic[int64] cancelled, Atomic[int64] cleaned):
    try:
        auto socket = tls.connect_resolved("localhost", "127.0.0.1", port, 0)
        tls.close_conn(socket)
    except Cancelled:
        cancelled.add(1)
    finally:
        cleaned.add(1)

def main():
    auto listener = net.listen("127.0.0.1", 0)
    auto port = net.port(listener)
    auto entered = channel[int64]()
    auto done = channel[int64]()
    spawn peer(listener, entered, done)
    Atomic[int64] cancelled = Atomic(0)
    Atomic[int64] cleaned = Atomic(0)
    auto failed = False
    try:
        scope:
            spawn dial(port, cancelled, cleaned)
            spawn trigger(entered)
    except Exception:
        failed = True
    auto eof = <-done
    if not failed or cancelled.load() != 1 or cleaned.load() != 1:
        raise ValueError("partial TLS dial was not cleaned before scope exit")
    print("cancelled handshake closes partial TLS connection")
