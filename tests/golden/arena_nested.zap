# Nested arenas (#553): the inner region is strictly shorter-lived than the
# outer; both free deterministically at their own block exit.
struct Rec:
    int64 v

def main():
    with arena() as outer:
        auto o = outer.new(Rec, 10)
        with arena() as inner:
            auto i = inner.new(Rec, 20)
            print(o.v + i.v)
        print(o.v)
