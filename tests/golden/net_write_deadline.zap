# The peer reads one byte, then stops consuming. A payload larger than loopback
# socket buffers must backpressure; one absolute budget bounds all partial writes.

def peer(bytes listener, channel[int64] release):
    auto socket = net.accept(listener)
    net.close_listener(listener)
    try:
        net.recv(socket, 1)
        auto stop = <-release
    finally:
        net.close_conn(socket)

def main():
    auto listener = net.listen("127.0.0.1", 0)
    auto port = net.port(listener)
    auto release = channel[int64]()
    spawn peer(listener, release)
    auto socket = net.connect_ip_deadline(
        "127.0.0.1",
        port,
        time.monotonic_ns() + 5000000000,
    )
    auto data = b"x"
    for i in range(0, 25):
        data = data + data
    net.set_deadline_ns(socket, time.monotonic_ns() + 100000000)
    auto timed_out = False
    try:
        net.send(socket, data)
    except TimeoutError:
        timed_out = True
    finally:
        net.close_conn(socket)
        release <- 1
    if not timed_out:
        raise ValueError("backpressured write was not bounded")
    print("backpressured TCP write deadline")
