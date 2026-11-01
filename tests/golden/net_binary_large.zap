# Exercise the Zap net/bytes boundary independently of the Redis connection.
def server(channel[int64] ports, int64 count):
    auto listener = net.listen("127.0.0.1", 0)
    ports <- net.port(listener)
    auto socket = net.accept(listener)
    auto received = b""
    while len(received) < count:
        auto chunk = bytes(net.recv(socket, 97))
        if len(chunk) == 0:
            raise ValueError("early EOF")
        received = received + chunk
    net.send(socket, received)
    net.close_conn(socket)
    net.close_listener(listener)

def main():
    auto payload = bytes([0, 255, 13, 10])
    auto i = 0
    while i < 14:
        payload = payload + payload
        i = i + 1
    auto ports = channel[int64]()
    spawn server(ports, len(payload))
    auto port = <-ports
    auto socket = net.connect("127.0.0.1", port)
    net.send(socket, payload)
    auto reply = b""
    while len(reply) < len(payload):
        auto chunk = bytes(net.recv(socket, 4096))
        if len(chunk) == 0:
            raise ValueError("early EOF")
        reply = reply + chunk
    net.close_conn(socket)
    if reply != payload:
        raise ValueError("binary payload changed")
    print("Binary TCP boundary: PASS")
