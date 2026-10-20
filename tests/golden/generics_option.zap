# Option[T] (I#154, D-0064): the flagship generic enum, shipped in the prelude.
# Construct Some/None, match, unwrap (raises on None — see
# generics_option_unwrap_raises), unwrap_or, is_some/is_none, for both an
# unboxed scalar payload (int64) and a pointer payload (str).
def describe(Option[int64] o) -> str:
    match o:
        case Option.Some(v):
            return "some"
        case Option.None:
            return "none"

def main():
    Option[int64] a = Option.Some(42)
    Option[int64] b = None
    print(a.unwrap())
    print(a.unwrap_or(0))
    print(b.unwrap_or(7))
    print(describe(a))
    print(describe(b))
    if a.is_some():
        print("a-some")
    if b.is_none():
        print("b-none")
    Option[str] s = Option.Some("hello")
    print(s.unwrap())
    Option[str] t = None
    print(t.unwrap_or("fallback"))
