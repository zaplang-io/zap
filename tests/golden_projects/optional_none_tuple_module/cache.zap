# A library module (#1195) exporting a function whose declared return type is a
# tuple of `Optional[bytes]` and which returns `(None, None)` on a miss — the
# pymemcache-style `gets(key)` shape. The bare `None` tuple elements must lower
# against the declared tuple type when the function is monomorphized in an
# *importing* module too (the modlink path), not only in its declaring module;
# the regression "prevents building a package consumer once the method is in
# the imported module".
def gets_miss() -> (Optional[bytes], Optional[bytes]):
    return (None, None)


def gets_hit() -> (Optional[bytes], Optional[bytes]):
    return (Optional.Some(b"value"), Optional.Some(b"42"))
