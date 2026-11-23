# Regression (#1324): a GENERIC free function with a parameter literally named
# `self` must have that parameter's type substituted per instantiation, exactly
# like any other parameter — it is an ordinary parameter, not a method receiver.
# Before the fix, monomorphization skipped type-substituting a `self`-named
# parameter (assuming it was a receiver bound via the owner type), so the
# specialized `g[Point]` kept `self: T`, and `reflect.field(self, …)` lowered to
# a field access on a non-struct value (ZAP6001). A non-generic `self` parameter
# was already fine; only the generic + reflect path broke.

struct Point:
    int64 x
    int64 y

def g[T](T self):
    comptime for f in reflect.type_info(T).fields:
        print(reflect.field(self, comptime f.index))

def main():
    g(Point(10, 20))
