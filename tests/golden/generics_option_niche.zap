# Niche vs tagged Option[T] layout (I#154, D-0064 #4). Option[str] and
# Option[Pair] use the niche layout (a single nullable pointer, no tag, no
# allocation — None is the null pointer, Some(p) is p). Option[int64] uses the
# tagged fallback (a value payload has no spare niche). Option[Option[str]] keeps
# the OUTER tagged — its payload is itself a niche optional, whose null would
# collide with the outer None — while the INNER stays niche. Behaviour is
# identical to the tagged representation; this fixture pins the decision boundary.
def main():
    Option[str] a = Option.Some("niche")
    Option[str] z = None
    Option[int64] b = Option.Some(7)
    Option[int64] n = None
    print(a.unwrap())
    print(a.unwrap_or("x"))
    print(z.unwrap_or("fallback"))
    print(b.unwrap())
    print(n.unwrap_or(42))

    # Nested: outer tagged, inner niche (Some case).
    Option[Option[str]] oo = Option.Some(Option.Some("deep"))
    match oo:
        case Option.Some(inner):
            print(inner.unwrap())
        case Option.None:
            print("outer-none")

    # Nested: outer tagged Some wrapping an inner niche None.
    Option[str] none_str = None
    Option[Option[str]] on = Option.Some(none_str)
    match on:
        case Option.Some(inner):
            print(inner.unwrap_or("inner-was-none"))
        case Option.None:
            print("outer-none")
