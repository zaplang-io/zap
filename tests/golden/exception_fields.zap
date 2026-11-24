# Golden fixture (#1432): a package-defined exception carries its declared
# fields across raise/catch. The catch binds the message string (unchanged from
# #1278, so `str(e)` / `print(e)` are identical), and `e.<field>` reads declared
# fields off the struct payload — a string, a non-string (int64), and an
# Optional — without parsing the diagnostic message. A message-only catch (via
# the built-in `Exception` base) still binds the message string.
@exception
struct TokenEndpointError:
    str message
    str error
    int64 status
    Optional[str] uri = None

def main() -> int64:
    # 1. A typed catch reads the string + int64 fields, and `print(e)` (the
    #    message binding) is unchanged.
    try:
        raise TokenEndpointError("request refused", "invalid_client", 401)
    except TokenEndpointError as failure:
        print("typed caught:")
        print(failure.error)
        print(failure.status)
        print(failure)

    # 2. An Optional field supplied as `Some(...)` survives the boundary.
    try:
        raise TokenEndpointError(
            "refused",
            "invalid_grant",
            400,
            Optional.Some("see docs"),
        )
    except TokenEndpointError as e:
        if e.uri.is_some():
            print(e.uri.unwrap())

    # 3. The Optional field's declared `None` default is preserved when omitted.
    try:
        raise TokenEndpointError("refused", "invalid_client", 401)
    except TokenEndpointError as e:
        if e.uri.is_none():
            print("uri defaulted none")

    # 4. A message-only catch (built-in `Exception` base) still binds the
    #    message string — the fields are additive, never required to read `e`.
    try:
        raise TokenEndpointError("just a message", "server_error", 500)
    except Exception as e:
        print("message-only:")
        print(e)

    return 0
