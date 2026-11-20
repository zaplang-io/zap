# Negative integer literals infer a generic type argument exactly as their
# positive counterparts do (#1117): the argument `-4` parses as a unary
# negation over an int literal, so monomorphization must type it `int64` to
# pin `Optional`'s `T`. This exercises the value round-trip (the exact `-4`
# survives `unwrap()`), the positive baseline, a user generic struct, and a
# negative literal passed to a method both positionally and by keyword.
struct Sampler:
    int seed

    def pick(self, Optional[int64] count = None) -> int64:
        return count.unwrap_or(0)

struct Box[T]:
    T item

def main():
    auto neg = Optional.Some(-4)
    print(neg.unwrap())
    auto pos = Optional.Some(4)
    print(pos.unwrap())
    auto fneg = Optional.Some(-2.5)
    print(fneg.unwrap())
    auto b = Box(-7)
    print(b.item)
    auto s = Sampler(0)
    print(s.pick(Optional.Some(-4)))
    print(s.pick(count=Optional.Some(-9)))
    print(s.pick())
