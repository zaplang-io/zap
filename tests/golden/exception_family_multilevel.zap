# Golden fixture (#1278): multi-level families are supported from day one — the
# runtime parent-chain walk is depth-agnostic. `NetworkError > DNSException >
# NXDOMAIN`: a grandparent clause catches a grandchild, a parent clause catches
# a child, and a grandparent clause catches the intermediate node.
@exception
struct NetworkError:
    str message

@exception(NetworkError)
struct DNSException:
    str message

@exception(DNSException)
struct NXDOMAIN:
    str message

def main() -> int64:
    # Grandparent catches grandchild (NXDOMAIN -> DNSException -> NetworkError).
    try:
        raise NXDOMAIN("deep")
    except NetworkError as e:
        print("grandparent caught grandchild:")
        print(e)

    # Parent catches child.
    try:
        raise NXDOMAIN("mid")
    except DNSException as e:
        print("parent caught child:")
        print(e)

    # Grandparent catches the intermediate node (DNSException -> NetworkError).
    try:
        raise DNSException("direct")
    except NetworkError as e:
        print("grandparent caught middle:")
        print(e)

    return 0
