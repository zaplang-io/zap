# claim: FN-7
# SPEC §7 (D-0095, I#496): a bare `*` separator makes every following parameter
# keyword-only — it may only be supplied by name at a call site (Python
# semantics). `mode` is a required keyword-only param; `tls` is a keyword-only
# param with a default that may be omitted. The leading `host` stays
# positional-or-keyword. Supplying a keyword-only param positionally, or omitting
# the required `mode`, is a compile-time error (sema ZAP4029 / ZAP4020), so those
# rejections live in the sema unit tests rather than this runnable fixture.
def code(str host, *, int64 mode, bool tls = False) -> int64:
    auto out = mode
    if tls:
        out = out + 1
    return out


def main():
    print(code("a", mode=80))
    print(code("a", mode=80, tls=True))
    print(code(host="a", mode=8080))
