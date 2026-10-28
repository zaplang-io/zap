# `_secret` is private (leading underscore, D-0028): reachable within `a`, but
# not through an import.
def _secret() -> int64:
    return 1
