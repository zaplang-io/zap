# Golden fixture (#1309): an interface method returning `Optional[int64]`, with
# a Some-returning and a None-returning struct conformer, consumed behind the
# interface type. A chained `Optional` method (`is_some`, `unwrap`) on the
# interface-call result must lower and run — before the fix it failed to lower
# with ZAP6001 "method call on a non-struct value" because the interface's
# return annotation was emitted un-mangled and the result lost its `Optional`
# struct type. This is the otel span/processor shape (zaplang-io/otel#15): a
# read-only span interface exposing an optional end timestamp a processor
# inspects, including the zero-timestamp case.

interface Reader:
    def end_time(self) -> Optional[int64]

struct EndedSpan:
    int64 t

    def end_time(self) -> Optional[int64]:
        return Optional.Some(self.t)

struct LiveSpan:
    def end_time(self) -> Optional[int64]:
        return Optional.None

def finished(Reader reader) -> bool:
    # Expression form: the Optional method is called directly on the result.
    return reader.end_time().is_some()

def end_or(Reader reader, int64 fallback) -> int64:
    # Local-binding form: the result is bound, then the method is called off it.
    auto t = reader.end_time()
    if t.is_some():
        return t.unwrap()
    return fallback

def main():
    Reader ended = EndedSpan(t=5)
    Reader live = LiveSpan()
    print(finished(ended))  # True
    print(finished(live))  # False
    print(end_or(ended, -1))  # 5
    print(end_or(live, -1))  # -1
    # The zero timestamp (otel#15): Some(0) is still present, not confused with
    # None — a scalar Optional is tagged, so the null niche never swallows a 0.
    Reader zero = EndedSpan(t=0)
    print(finished(zero))  # True
    print(end_or(zero, -1))  # 0
