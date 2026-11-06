# Golden fixture: a niche-layout `Optional[T]` (pointer payload) as a dict key
# (#704, D-0122). `Optional[str]` uses the null-niche layout (D-0064 #4): the
# value IS the payload pointer, `None` is the null pointer. The synthesized
# key `__hash__` tests `None` with a null check (not a discriminant tag) and
# hashes a present payload by its bytes (`zap_str_hash`); `__eq__` keeps `None`
# distinct from every present string. Exercises insert, a bare `None` key,
# index-read, structural overwrite, and membership.
def main():
    dict[Optional[str], int64] d = {}
    Optional[str] k = Optional.Some("hi")
    d[k] = 1
    d[None] = 2
    print(len(d))  # 2
    print(d[k])  # 1
    print(d[None])  # 2
    # A separately built equal Some finds the same slot (compared by bytes).
    Optional[str] k2 = Optional.Some("hi")
    d[k2] = 10
    print(len(d))  # 2
    print(d[k])  # 10
    print(k in d)  # True
    print(None in d)  # True
    print(Optional.Some("bye") in d)  # False
