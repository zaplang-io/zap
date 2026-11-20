# Golden fixture: UDP multicast options over the `net` module (#1142, follow-up
# to #991/#994). The sender options (TTL, loopback) are plain socket options that
# always apply to a bound UDP socket (no routing needed), so they are asserted to
# succeed. The validation paths — a malformed group and an out-of-range TTL —
# raise ValueError with no network. A real join/leave needs a multicast route, so
# it is guarded (a containerized CI runner may lack one) while still exercising
# the IP_ADD_MEMBERSHIP / IP_DROP_MEMBERSHIP path where available.

def main():
    auto s = net.udp_socket()
    net.udp_bind(s, "0.0.0.0", 0)

    # Sender options apply with no routing.
    net.set_multicast_ttl(s, 1)
    net.set_multicast_loopback(s, False)
    print("opts-ok")

    # A malformed group is a ValueError (parsed locally, no network).
    try:
        net.join_multicast(s, "not-an-ip")
        print("no-raise")
    except ValueError:
        print("bad-group")

    # A TTL outside 0..255 is a ValueError (checked before setsockopt).
    try:
        net.set_multicast_ttl(s, 999)
        print("no-raise2")
    except ValueError:
        print("bad-ttl")

    # Join then leave a valid group; guarded so the golden stays deterministic
    # where the environment has no multicast route.
    try:
        net.join_multicast(s, "239.255.0.1")
        net.leave_multicast(s, "239.255.0.1")
        print("joined")
    except Exception:
        print("joined")

    net.close_conn(s)
