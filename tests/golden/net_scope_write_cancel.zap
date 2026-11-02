# With one worker the writer must park before peer/trigger can run. With several
# workers this also covers cancellation racing with a successful partial write.

def peer(bytes listener, channel[int64] entered, channel[int64] release):
    auto socket = net.accept(listener)
    net.close_listener(listener)
    try:
        net.recv(socket, 1)
        entered <- 1
        auto stop = <-release
    finally:
        net.close_conn(socket)

def trigger(channel[int64] entered):
    auto started = <-entered
    raise ValueError("cancel write fixture")

def writer(bytes socket, bytes payload, Atomic[int64] cleaned, Atomic[int64] cancelled):
    try:
        net.send(socket, payload)
    except Cancelled:
        cancelled.add(1)
    finally:
        net.close_conn(socket)
        cleaned.add(1)

def main():
    auto payload = b"x"
    for i in range(0, 25):
        payload = payload + payload
    Atomic[int64] cleaned = Atomic(0)
    Atomic[int64] cancelled = Atomic(0)
    for i in range(0, 4):
        auto listener = net.listen("127.0.0.1", 0)
        auto port = net.port(listener)
        auto entered = channel[int64]()
        auto release = channel[int64]()
        spawn peer(listener, entered, release)
        auto socket = net.connect("127.0.0.1", port)
        auto failed = False
        try:
            scope:
                spawn writer(socket, payload, cleaned, cancelled)
                spawn trigger(entered)
        except Exception:
            failed = True
            if cleaned.load() != i + 1 or cancelled.load() != i + 1:
                raise ValueError("write cleanup did not precede scope exit")
        if not failed:
            raise ValueError("expected writer cancellation")
        release <- 1
    print("cancelled TCP writes cleaned before scope exit")
