# Golden: bounds referencing other type parameters (#1206).
#
# A parametrized bound may reference the type parameter it constrains:
# `[T: Scaler[f64, T]]` says "T can be scaled by an f64 and the result is a T".
# This is the generics shape the reflected operator-capability bound #1199 needs
# (`[T: RMul[f64, T]]`), modeled here with a plain `scale` method to stay
# generics-only (no operator machinery).
#
# Inside the body the call THROUGH the type parameter — `x.scale(k)` — resolves
# the interface's substituted signature (In := f64, Out := T), so its result is
# typed `T` and flows back out of the generic as `T` (#1206, building on the
# generic-interface conformance of #1205). Conformance is enforced per
# instantiation by the monomorphizer (static dispatch, no fat pointer): `T` is
# substituted into the bound first (`Scaler[f64, Meters]`) and the concrete type
# is matched against it. GC is unaffected — bounds add no runtime mechanism.

interface Scaler[In, Out]:
    def scale(self, In factor) -> Out

value struct Meters:
    f64 v
    def scale(self, f64 factor) -> Meters:
        return Meters(v=self.v * factor)

# Self-referential bound + a call through the type parameter whose result is T.
def scaled[T: Scaler[f64, T]](T x, f64 k) -> T:
    return x.scale(k)

def main():
    Meters a = scaled(Meters(v=3.0), 4.0)
    print(a.v)                                  # 12.0
    # Nested: the inner call returns a Meters, which the outer call scales again.
    Meters b = scaled(scaled(Meters(v=1.0), 2.0), 5.0)
    print(b.v)                                  # 10.0
