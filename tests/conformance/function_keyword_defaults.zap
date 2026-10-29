# claim: FN-2, FN-3
# SPEC §7 (D-0038): a parameter may be passed by name (FN-2); positional args
# precede keyword args at a call site, and any parameter with a default may be
# omitted (FN-3). `code()` returns the port, +1 when tls is on, so each call's
# result shows which arguments bound where.
def code(str host, int64 port = 8080, bool tls = False) -> int64:
    auto out = port
    if tls:
        out = out + 1
    return out


def main():
    print(code("a"))
    print(code("a", tls=True))
    print(code(host="a", port=80))
    print(code("a", 443, True))
