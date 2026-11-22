# Golden fixture (#1278): a family root is not a distinct kind — it is an
# ordinary `@exception`, itself raiseable and caught by its own name. The edge
# is one-directional: `except Member` does NOT catch the root (the root is not
# in the member's chain), so a following `except Root` clause handles it.
@exception
struct DNSException:
    str message

@exception(DNSException)
struct NXDOMAIN:
    str message

def main() -> int64:
    # The root raises and is caught by its own name.
    try:
        raise DNSException("root raised")
    except DNSException as e:
        print("root caught:")
        print(e)

    # A member clause does not catch the root; the base clause after it does.
    try:
        raise DNSException("not a member")
    except NXDOMAIN as e:
        print("UNREACHED: root is not an NXDOMAIN")
    except DNSException as e:
        print("root not caught by member clause:")
        print(e)

    return 0
