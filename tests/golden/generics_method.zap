# Generic methods with their own type parameters (I#161): on a generic enum
# (Option[T].and_opt[U] — Rust's Option::and: returns `other` iff self is Some)
# and on a generic struct (Box[T].repack[U]). Each specialized method is emitted
# onto its owner's concrete instance (Option$int64, Box$int64, ...) and
# dispatched directly through the ordinary struct-method path. Explicit method
# type arguments (repack[int64]) exercise the explicit path.
struct Box[T]:
    T value
    def get(self) -> T:
        return self.value
    def repack[U](self, U y) -> Box[U]:
        return Box(y)

def main():
    Option[int64] a = Option.Some(1)
    Option[str] b = Option.Some("yes")
    print(a.and_opt(b).unwrap_or("no"))
    Option[int64] n = None
    print(n.and_opt(b).unwrap_or("defaulted"))
    Box[int64] bx = Box(3)
    print(bx.get())
    Box[str] bs = bx.repack("swapped")
    print(bs.get())
    print(bx.repack[int64](99).get())
