# A library module exporting a value struct with `Optional[T]` fields (#1262).
# A bare payload supplied at construction must auto-wrap to a tagged `Some`
# through the modlink path when the struct is constructed in an *importing*
# module, exactly as in the declaring module — the cross-module half of the
# regression.
value struct Point:
    Optional[int64] x = None
    Optional[int64] y = None
    Optional[str] label = None
