# Niche vs tagged Optional[T] layout (I#154, D-0064 #4). Optional[str] and
# Optional[Pair] use the niche layout (a single nullable pointer, no tag, no
# allocation — None is the null pointer, Some(p) is p). Optional[int64] uses the
# tagged fallback (a value payload has no spare niche). Optional[Optional[str]] keeps
# the OUTER tagged — its payload is itself a niche optional, whose null would
# collide with the outer None — while the INNER stays niche. Behaviour is
# identical to the tagged representation; this fixture pins the decision boundary.
def main():
    Optional[str] a = Optional.Some("niche")
    Optional[str] z = None
    Optional[int64] b = Optional.Some(7)
    Optional[int64] n = None
    print(a.unwrap())
    print(a.unwrap_or("x"))
    print(z.unwrap_or("fallback"))
    print(b.unwrap())
    print(n.unwrap_or(42))

    # Nested: outer tagged, inner niche (Some case).
    Optional[Optional[str]] oo = Optional.Some(Optional.Some("deep"))
    match oo:
        case Optional.Some(inner):
            print(inner.unwrap())
        case Optional.None:
            print("outer-none")

    # Nested: outer tagged Some wrapping an inner niche None.
    Optional[str] none_str = None
    Optional[Optional[str]] on = Optional.Some(none_str)
    match on:
        case Optional.Some(inner):
            print(inner.unwrap_or("inner-was-none"))
        case Optional.None:
            print("outer-none")
