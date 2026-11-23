# Consumer (#1309): construct the imported conformers behind the imported
# interface type and drive both the Some and None branches through the imported
# consumers. Before the fix this failed lowering with ZAP6001 "method call on a
# non-struct value"; the interface call's `Optional[int64]` result now keeps its
# concrete struct type across the import boundary, so the chained Optional
# methods lower and run.
import spans


def main():
    spans.Reader ended = spans.EndedSpan(t=5)
    spans.Reader live = spans.LiveSpan()
    print(spans.finished(ended))  # True
    print(spans.finished(live))  # False
    print(spans.end_or(ended, -1))  # 5
    print(spans.end_or(live, -1))  # -1
    # The zero timestamp (otel#15): Some(0) stays present across the import.
    spans.Reader zero = spans.EndedSpan(t=0)
    print(spans.finished(zero))  # True
    print(spans.end_or(zero, -1))  # 0
