# Cross-module structured exception fields (#1432): a package-defined exception
# raised inside an imported module is caught by its qualified type in the
# importer, which reads the declared fields (`e.error`, `e.status`) off the
# payload — no diagnostic-message parsing, across the module boundary. The type
# id and struct layout are the consistently-mangled symbol's, so the member the
# dependency raises is exactly the type the importer catches.
import resolver

def main() -> int64:
    # A value constructed here, caught by the qualified type, reading fields.
    try:
        raise resolver.TokenEndpointError("local refusal", "invalid_client", 400)
    except resolver.TokenEndpointError as e:
        print("xmod typed (local):")
        print(e.error)
        print(e.status)

    # Raised deep inside the imported module, caught here reading fields and the
    # message binding.
    try:
        auto n = resolver.request_token("unauthorized_client")
        print(n)
    except resolver.TokenEndpointError as e:
        print("xmod typed (dep raise):")
        print(e.error)
        print(e.status)
        print(e)

    return 0
