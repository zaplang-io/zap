# Golden fixture: forward DNS resolution over the `net` module (#992, epic
# #985). Numeric-literal inputs pass through getaddrinfo unchanged, so the
# results are deterministic and need no network (no real DNS query): a numeric
# host resolves to exactly itself.
#   - net.lookup_host(name) -> list[str] of numeric IPs (A + AAAA)
#   - net.lookup_ipv4(name) restricts to IPv4
#   - net.lookup_addr(ip) raises ValueError on a non-numeric IP (no network)

def main():
    auto v4 = net.lookup_host("127.0.0.1")
    print(len(v4))  # 1
    print(v4[0])  # 127.0.0.1

    auto only4 = net.lookup_ipv4("127.0.0.1")
    print(len(only4))  # 1
    print(only4[0])  # 127.0.0.1

    # A malformed IP to the reverse lookup is a ValueError, resolved locally
    # (inet_pton fails) with no network.
    try:
        auto names = net.lookup_addr("not-an-ip")
        print("no-raise")
    except ValueError:
        print("bad-ip")
