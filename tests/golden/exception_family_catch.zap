# Golden fixture (#1278): package-defined exception families. `@exception`
# marks a catchable exception; `@exception(DNSException)` groups a member under
# it, so `except DNSException` catches the whole family. A more specific clause
# still wins when it comes first (source order), and the built-in `Exception`
# still catches any user member. `e` in `except E as e` is the message string.
@exception
struct DNSException:
    str message

@exception(DNSException)
struct NXDOMAIN:
    str message

@exception(DNSException)
struct ServFail:
    str message

def main() -> int64:
    # 1. The family base catches a member.
    try:
        raise NXDOMAIN("no such domain")
    except DNSException as e:
        print("family caught:")
        print(e)

    # 2. A concrete clause placed before the base wins (first match in order).
    try:
        raise NXDOMAIN("nx again")
    except NXDOMAIN as e:
        print("concrete first:")
        print(e)
    except DNSException:
        print("UNREACHED base")

    # 3. A different member of the same family, caught by the shared base.
    try:
        raise ServFail("server failure")
    except DNSException as e:
        print("family caught 2:")
        print(e)

    # 4. The built-in `Exception` still catches a user member (its chain ends
    #    at Exception -> BaseException, the family edge notwithstanding).
    try:
        raise NXDOMAIN("via Exception")
    except Exception as e:
        print("exception caught:")
        print(e)

    return 0
