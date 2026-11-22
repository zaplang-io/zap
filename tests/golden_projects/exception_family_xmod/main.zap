# Cross-module exception families (#1278): a member raised inside an imported
# package is caught by the qualified family base in the importer. The type ids
# are name-hashes of the consistently-mangled symbols, so the family edge
# registered at startup threads the member through its base across the module
# boundary — exactly as a local family does.
import resolver

def main() -> int64:
    # A member value constructed here, caught by the qualified base.
    try:
        raise resolver.NXDOMAIN("local member")
    except resolver.DNSException as e:
        print("xmod base caught local member:")
        print(e)

    # A member raised deep inside the imported module, caught by the base here.
    try:
        auto n = resolver.lookup("example.com")
        print(n)
    except resolver.DNSException as e:
        print("xmod base caught dep raise:")
        print(e)

    # The qualified member clause still catches its own concrete type.
    try:
        raise resolver.ServFail("downstream")
    except resolver.ServFail as e:
        print("xmod concrete caught:")
        print(e)

    return 0
