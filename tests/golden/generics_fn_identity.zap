# Generic free functions (I#161, slice 4): a generic identity fn instantiated at
# multiple type-arg sets proves monomorphization + per-tuple dedup (identity$int64
# is emitted once and reused for the repeat; a distinct T gets a distinct
# instance). Explicit call-site type arguments exercise the explicit path (Fork
# S); argument-driven inference (Fork I) covers the rest.
def identity[T](T x) -> T:
    return x

def main():
    print(identity(5))
    print(identity("hi"))
    print(identity(9))  # dedup: reuses identity$int64
    print(identity[int64](7))  # explicit type args
    print(identity[str]("x"))  # explicit type args
