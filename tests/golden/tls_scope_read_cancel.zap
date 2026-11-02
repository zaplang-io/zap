def trigger(channel[int64] entered):
    auto started = <-entered
    raise ValueError("cancel TLS fixture")

def reader(
    bytes socket,
    channel[int64] entered,
    Atomic[int64] cancelled,
    Atomic[int64] cleaned,
):
    try:
        entered <- 1
        tls.recv(socket, 1)
    except Cancelled:
        cancelled.add(1)
    finally:
        tls.close_conn(socket)
        cleaned.add(1)

def main():
    auto port = tls._test_server()
    auto socket = tls.connect_insecure("127.0.0.1", port)
    auto entered = channel[int64]()
    Atomic[int64] cancelled = Atomic(0)
    Atomic[int64] cleaned = Atomic(0)
    try:
        scope:
            spawn reader(socket, entered, cancelled, cleaned)
            spawn trigger(entered)
    except Exception:
        if cancelled.load() != 1 or cleaned.load() != 1:
            raise ValueError("TLS cancellation cleanup failed")
    print("cancelled TLS read cleaned before scope exit")
