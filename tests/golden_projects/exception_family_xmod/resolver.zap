# A sibling module (#1278) exporting an exception family and a function that
# raises a member of it. The modlink pass must rewrite the `@exception(...)`
# parent reference to the module's mangled symbol, so an importer's
# `except resolver.DNSException` matches a member raised inside this module.
@exception
struct DNSException:
    str message

@exception(DNSException)
struct NXDOMAIN:
    str message

@exception(DNSException)
struct ServFail:
    str message

# Raises a family member from inside this module.
def lookup(str host) -> int64:
    raise NXDOMAIN(host)
