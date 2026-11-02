# Validate expired bounds before ready I/O, then clear and reuse the connection.

def peer(bytes listener, channel[int64] ready):
    auto socket = net.accept(listener)
    net.close_listener(listener)
    try:
        net.send(socket, b"ready")
        ready <- 1
        net.recv(socket, 1)
    finally:
        net.close_conn(socket)

def main():
    auto listener = net.listen("127.0.0.1", 0)
    auto port = net.port(listener)
    auto ready = channel[int64]()
    spawn peer(listener, ready)
    auto socket = net.connect_ip_deadline(
        "127.0.0.1",
        port,
        time.monotonic_ns() + 5000000000,
    )
    auto sent = <-ready
    auto errors = 0
    try:
        net.set_deadline_ns(socket, -1)
    except ValueError:
        errors = errors + 1
    net.set_deadline_ns(socket, 1)
    try:
        net.recv(socket, 5)
    except TimeoutError:
        errors = errors + 1
    try:
        net.send(socket, b"x")
    except TimeoutError:
        errors = errors + 1
    net.set_deadline_ns(socket, 0)
    if net.recv(socket, 5) != "ready":
        raise ValueError("deadline consumed ready bytes")
    net.send(socket, b"x")
    net.close_conn(socket)
    try:
        net.connect_ip_deadline("localhost", port, time.monotonic_ns() + 1000000000)
    except ValueError:
        errors = errors + 1
    try:
        net.connect_ip_deadline("127.0.0.1", port, 1)
    except TimeoutError:
        errors = errors + 1
    try:
        net.connect_ip_deadline("127.0.0.1", port, -1)
    except ValueError:
        errors = errors + 1
    if errors != 6:
        raise ValueError("deadline validation failed")
    print("absolute read/write deadline validates, persists, and clears")
    print("bounded connect rejects DNS and expired budgets")
