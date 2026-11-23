# Library module (#1309): an interface whose method returns `Optional[int64]`,
# two conformers (a Some-returning and a None-returning span), and consumers
# that call the Optional method off the interface-typed value. Declared in an
# imported module so whole-program lowering mangles the interface (and its
# method's `Optional[int64]` return) with the module prefix — the issue notes
# the ZAP6001 "method call on a non-struct value" failure also occurs when the
# interface is imported from another module. This is the otel span/processor
# shape (zaplang-io/otel#15): a read-only span interface exposing an optional
# end timestamp an SDK processor inspects.
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
    return reader.end_time().is_some()


def end_or(Reader reader, int64 fallback) -> int64:
    auto t = reader.end_time()
    if t.is_some():
        return t.unwrap()
    return fallback
