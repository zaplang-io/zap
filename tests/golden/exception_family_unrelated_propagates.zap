# Golden fixture (#1278): `except Base` catches only Base's own family. An
# exception from an unrelated `@exception` family is NOT caught and propagates
# past the handler — here out of `main`, so the process exits non-zero and
# stderr names the uncaught message (.exit / .stderr sidecars).
@exception
struct DNSException:
    str message

@exception(DNSException)
struct NXDOMAIN:
    str message

# A separate, unrelated exception family (not under DNSException).
@exception
struct HTTPException:
    str message

def main() -> int64:
    print("before")
    try:
        raise HTTPException("500 server error")
    except DNSException as e:
        print("UNREACHED: HTTPException is not a DNSException")
    print("after")  # unreachable — HTTPException escaped uncaught
    return 0
