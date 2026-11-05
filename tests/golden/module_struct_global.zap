# Module-level globals of a user STRUCT type (#689, epic #661 / D-0112): field
# reads and method calls on const and mutable struct globals, mutation of a
# mutable struct global's field, and a nested struct-in-struct field — all
# resolve against the struct value held in the global slot, exactly as a local
# of the same type does.
struct Acc:
    int64 n

    def get(self) -> int64:
        return self.n

struct Inner:
    int64 v

struct Outer:
    Inner inner

const Acc C = Acc(5)
Acc M = Acc(10)
const Outer O = Outer(Inner(7))

def main():
    print(C.n)  # 5  — const struct global field read
    print(C.get())  # 5  — const struct global method call
    print(M.n)  # 10 — mutable struct global field read
    print(M.get())  # 10 — mutable struct global method call
    M.n = 42
    print(M.n)  # 42 — mutation of a mutable struct global's field
    print(O.inner.v)  # 7  — nested struct field access
