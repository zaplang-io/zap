# Golden fixture: a `value struct` boxed behind an interface renders correctly
# through the renderer slot (#707, D-0124, with I#75/D-0051). On the concrete ->
# interface conversion the value struct is boxed (copied to the heap); rendering
# the interface value dispatches to the concrete type's renderer over that boxed
# copy, so it shows the snapshot taken at the conversion — independent of a later
# mutation of the original, exactly as method dispatch sees the boxed copy.

interface Shape:
    def area(self) -> f64

value struct Rect:
    f64 w
    f64 h

    def area(self) -> f64:
        return self.w * self.h

def main():
    auto r = Rect(w=2.0, h=3.0)
    Shape s = r
    r.w = 100.0  # mutate the ORIGINAL after it was boxed behind `s`
    print(s)  # Rect(w=2.0, h=3.0)    — the boxed copy is unaffected
    print(r)  # Rect(w=100.0, h=3.0)  — the concrete original did change
