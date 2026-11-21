# Dependency `memcache` (D-0052), pre-materialized into zenv/pkg so the fixture
# needs no network or resolve step. Exports a struct whose method takes a
# variadic `bytes... args` parameter (#1218) — modeling the
# `Client.stats(bytes... args)` API of zaplang-io/memcached#7, whose native
# build first surfaced this bug (the importing-package / cross-module case).
struct Client:
    int64 offset

    def stats(self, bytes... args) -> int64:
        return self.offset + len(args)


def connect(int64 off) -> Client:
    return Client(offset=off)
