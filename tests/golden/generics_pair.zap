# A user generic struct Pair[A, B] instantiated at TWO different type-arg sets
# (proves monomorphization produces distinct concrete layouts) with a repeat of
# the first set (proves dedup), plus a nested generic Option[Pair[int64, str]].
struct Pair[A, B]:
    A first
    B second
    def get_first(self) -> A:
        return self.first
    def get_second(self) -> B:
        return self.second

def main():
    Pair[int64, str] p = Pair(1, "x")
    Pair[str, int64] q = Pair("y", 2)
    Pair[int64, str] r = Pair(3, "z")
    print(p.get_first())
    print(p.get_second())
    print(q.get_first())
    print(q.get_second())
    print(r.get_first())
    Option[Pair[int64, str]] op = Option.Some(Pair(9, "nested"))
    match op:
        case Option.Some(pp):
            print(pp.get_first())
            print(pp.get_second())
        case Option.None:
            print("none")
