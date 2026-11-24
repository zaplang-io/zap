# A sibling module (#1432) exporting a package-defined exception that carries
# structured fields, and a function that raises it with those fields populated.
# modlink rewrites the raise-site struct reference and the importer's typed
# `except` clause to the same mangled symbol, so the declared fields the member
# carries on its payload are readable in the importer's typed catch across the
# module boundary — a dependency of the OAuth token-endpoint error (RFC 6749
# §5.2), raised inside the package and caught by the application.
@exception
struct TokenEndpointError:
    str message
    str error
    int64 status

# Raises the exception with its structured fields from inside this module.
def request_token(str grant) -> int64:
    raise TokenEndpointError("request refused", grant, 401)
