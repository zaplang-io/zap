# Higher-order generics (I#163 x I#161): a generic function whose parameter is a
# function value `fn(T)->U`. Monomorphization specializes both the value types
# and the function-value signature, so `apply` instantiates at (int64,int64) and
# (str,int64). Exercised with a top-level function AND a lambda as the callback.
def apply[T, U](T x, fn(T)->U f) -> U:
    return f(x)

def double(int64 n) -> int64:
    return n + n

def shout(str s) -> int64:
    return len(s)

def main():
    print(apply[int64, int64](21, double))
    print(apply[int64, int64](10, lambda v: v * 3))
    print(apply[str, int64]("hello", shout))
