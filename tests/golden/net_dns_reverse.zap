# Golden fixture: reverse DNS resolution over the `net` module (#992, epic
# #985). net.lookup_addr(ip) returns the PTR hostname(s) of a numeric IP as a
# list[str] (empty when the address has no reverse record). The reverse of the
# loopback address is resolved from the hosts database (no network), and the
# exact name varies by host, so this asserts only that a valid numeric IP does
# not raise and yields a list — the malformed-IP ValueError path is pinned by
# net_dns_lookup.

def main():
    auto names = net.lookup_addr("127.0.0.1")
    # A valid IP never raises; the result is a (possibly empty) list of names.
    auto n = len(names)
    if n >= 0:
        print("reverse-ok")
