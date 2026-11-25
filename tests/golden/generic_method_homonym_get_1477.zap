# Regression for #1477: a user type's own `get` method must never be hijacked
# by the builtin `dict.get(key) -> Optional[V]` special-casing. That rewrite
# used the call's expected `Optional[..]` sink as a fallback value type, which
# latched onto ANY `.get` whose binding/return type was `Optional[..]` — even a
# user struct/enum with its own `get` — and forged a bogus `get$Optional$V`
# symbol that no method existed under, aborting IR lowering with
# `ZAP6001: no method 'get$Optional$bytes' on that value`. A generic homonym
# `Typed[T].get` is not required to trigger it; the expected-type context alone
# does. This exercises: the exact issue shape (generic `Typed[T].get` coexisting
# with a concrete `Client.get`, reached through a field receiver
# `lease.client.get`), a user struct `get` in both an `Optional[..]` binding and
# a `return` context, one- and two-argument forms, a user enum method named
# `get`, AND a genuine `dict.get` in the same program that must still specialize
# normally (pointer `bytes` and scalar `int64` payloads). Unblocks
# zaplang-io/memcached#9.


struct Typed[T]:
    T value

    def get(self, bytes key, Optional[T] default = None) -> Optional[T]:
        return default


struct Client:
    int64 unused

    def get(self, bytes key, Optional[bytes] default = None) -> Optional[bytes]:
        return default


struct Lease:
    Client client


struct Pool:
    Client client

    def get(self, bytes key, Optional[bytes] default = None) -> Optional[bytes]:
        auto lease = Lease(self.client)
        return lease.client.get(key, default)


enum Store:
    Empty
    Full(bytes v)

    def get(self, bytes key) -> Optional[bytes]:
        match self:
            case Store.Full(v):
                return Optional.Some(v)
            case Store.Empty:
                return None


# A free function whose declared return type supplies the `Optional[bytes]`
# context that used to forge the bad symbol.
def fetch(Client c) -> Optional[bytes]:
    return c.get(b"k", Optional.Some(b"ret"))


def main():
    # The generic homonym is instantiated but must not poison concrete dispatch.
    auto typed = Typed[str]("present")
    auto ignored = typed.get(b"k")
    print(ignored.is_none())  # True

    # Field-receiver dispatch through `Pool.get` -> `Lease.client.get`: the
    # original reproducer. Two-arg form, result consumed by `.unwrap()`.
    auto pool = Pool(Client(0))
    print(pool.get(b"k", Optional.Some(b"fallback")).unwrap())  # fallback

    # Two-arg user `get` bound to an explicit `Optional[bytes]` local.
    auto client = Client(0)
    Optional[bytes] bound = client.get(b"k", Optional.Some(b"bound"))
    print(bound.unwrap())  # bound

    # One-arg user `get` bound to `Optional[bytes]` (defaulted `None`).
    Optional[bytes] one = client.get(b"k")
    print(one.is_none())  # True

    # Return-context user `get`.
    print(fetch(client).unwrap())  # ret

    # A user enum method named `get`.
    auto full = Store.Full(b"stored")
    print(full.get(b"k").unwrap())  # stored
    auto empty = Store.Empty
    print(empty.get(b"k").is_none())  # True

    # A genuine `dict.get` in the SAME program must still specialize — pointer
    # payload (bytes values) and scalar payload (int64 values), both the
    # one-arg `Optional` form and the two-arg plain-value form.
    dict[bytes, bytes] words = {}
    words[b"a"] = b"alpha"
    Optional[bytes] wa = words.get(b"a")
    print(wa.unwrap())  # alpha
    print(words.get(b"z").is_none())  # True
    print(words.get(b"z", b"def"))  # def

    dict[bytes, int64] counts = {}
    counts[b"a"] = 7
    Optional[int64] ca = counts.get(b"a")
    print(ca.unwrap())  # 7
    print(counts.get(b"z", -1))  # -1
