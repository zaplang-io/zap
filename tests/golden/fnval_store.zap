# Storing function values (I#163): a `fn(int64)->int64` value is kept in a struct
# field and in a `list` element, then invoked after being retrieved. A struct
# field preserves the full function type; a list element is retrieved into a
# `fn(...)`-annotated binding (collection elements are stored as uniform
# reference words, so the annotation re-attaches the call signature).
struct Op:
    fn(int64) -> int64 f
    str name

def inc(int64 x) -> int64:
    return x + 1

def dec(int64 x) -> int64:
    return x - 1

def main():
    Op op = Op(inc, "inc")
    auto g = op.f
    print(op.name, g(41))

    auto ops = [inc, dec]
    fn(int64) -> int64 g0 = ops[0]
    fn(int64) -> int64 g1 = ops[1]
    print(g0(10), g1(10))
