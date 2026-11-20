# Golden fixture: variadic parameters on a STRUCT METHOD (#1218, D-0038). A
# `TYPE... name` method parameter is "received as a `list`" (SPEC §7) exactly
# like a free function's: the callee iterates it, takes its `len`, and indexes
# it, and the caller packs the trailing arguments into that one list — so a bare
# `recv.m()` passes an EMPTY list, not nothing. The defect (#1218) was that the
# method call site forwarded the receiver alone (or the receiver plus each
# vararg separately), an argument-count mismatch the native backend rejected
# with "Incorrect number of arguments passed to called function!".
#
# Covers, for a struct method: zero / one / many trailing arguments; a receiver
# whose own field state (`self.offset`) is threaded alongside the collected
# list; element USE (iteration and `len` per element, and indexing) to prove the
# arguments become real list items and not just a count; and a fixed parameter
# preceding the variadic.
struct Counter:
    int64 offset

    def count(self, bytes... args) -> int64:
        return self.offset + len(args)

    def total_len(self, bytes... parts) -> int64:
        auto n = self.offset
        for p in parts:
            n = n + len(p)
        return n

    def tally(self, int64 base, bytes... parts) -> int64:
        return base + len(parts)

    def first_len(self, bytes... parts) -> int64:
        return len(parts[0])


def main():
    auto c0 = Counter(0)
    auto c10 = Counter(10)
    # Zero / one / many — the bare call materializes an empty list.
    print(c0.count())
    print(c0.count(b"a"))
    print(c0.count(b"a", b"b"))
    print(c0.count(b"a", b"b", b"c"))
    # Receiver state is still threaded alongside the collected list.
    print(c10.count())
    print(c10.count(b"x", b"y"))
    # Elements are real list items: iterate and take each element's `len`.
    print(c0.total_len())
    print(c0.total_len(b"ab", b"cde"))
    print(c10.total_len(b"ab", b"cde"))
    # A fixed parameter preceding the variadic.
    print(c0.tally(100))
    print(c0.tally(100, b"a", b"b"))
    # Indexing the collected list.
    print(c0.first_len(b"hello"))
    print(c0.first_len(b"hi", b"there"))
