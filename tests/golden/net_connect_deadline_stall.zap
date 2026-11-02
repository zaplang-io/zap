# Saturate a loopback listener that never accepts. Unlike an unroutable external
# address, backlog saturation produces an actual stalled connect on Linux CI.

def main():
    auto listener = net.listen("127.0.0.1", 0)
    auto port = net.port(listener)
    list[bytes] sockets = []
    auto timed_out = False
    try:
        for i in range(0, 512):
            try:
                auto socket = net.connect_ip_deadline(
                    "127.0.0.1",
                    port,
                    time.monotonic_ns() + 100000000,
                )
                sockets.append(socket)
            except TimeoutError:
                timed_out = True
                break
        if not timed_out:
            raise ValueError("listener backlog did not stall a connect")
        # Repeat failed connects to exercise partial-fd/poller cleanup.
        for i in range(0, 4):
            auto failed = False
            try:
                auto socket = net.connect_ip_deadline(
                    "127.0.0.1",
                    port,
                    time.monotonic_ns() + 20000000,
                )
                sockets.append(socket)
            except TimeoutError:
                failed = True
            if not failed:
                raise ValueError("expected saturated connect timeout")
    finally:
        for socket in sockets:
            net.close_conn(socket)
        net.close_listener(listener)
    print("stalled numeric connects time out with partial sockets cleaned")
