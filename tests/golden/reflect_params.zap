# Golden fixture: function/callable parameter reflection (I#458, D-0089).
#
# #26/D-0036 shipped struct/type reflection and #457/D-0087 keyed it to functions;
# both expose only decorator/annotation names. #458 adds *signature* reflection —
# a function/callable's parameter names, declared types, defaults, and per-param
# annotations — the substrate FastAPI-style typed injection and `Depends(...)` DI
# need (fastzap#7). All queries are string-keyed like reflect_has/reflect_names:
# the owner is a function name (top-level `def`) or "Type.method".
#
#   reflect_params(owner)              -> comma-joined param names (decl order)
#   reflect_sig(owner)                 -> readable one-line signature
#   reflect_param_type(owner, p)       -> rendered declared type ("" if untyped)
#   reflect_param_has_default(owner,p) -> bool
#   reflect_param_annotations(owner,p) -> comma-joined annotation names ("" none)
#
# A *declared* annotation on a parameter (`@Inject` below) is recorded as
# reflectable metadata (the fn-level model, extended to params); the implicit
# `self` receiver is excluded.

annotation Inject:
    str service = ""

struct Svc:
    int64 n

    def handle(self, int64 id, str q = "x") -> int64:
        return id

def plain(int64 a, int64 b) -> int64:
    return a + b

def greet(str name, @Inject Svc dep, int64 times = 3) -> str:
    return name

def main():
    # Plain function: names + signature, no defaults.
    print(reflect_params("plain"))
    print(reflect_sig("plain"))

    # Annotated + defaulted function.
    print(reflect_params("greet"))
    print(reflect_sig("greet"))
    print(reflect_param_type("greet", "name"))
    print(reflect_param_type("greet", "dep"))
    print(reflect_param_annotations("greet", "dep"))
    if reflect_param_has_default("greet", "times"):
        print("times has default")
    if not reflect_param_has_default("greet", "name"):
        print("name has no default")

    # Struct method: keyed "Type.method"; self excluded.
    print(reflect_params("Svc.handle"))
    print(reflect_sig("Svc.handle"))

    # Unknown owner / param: empty + false.
    print(reflect_params("missing"))
    if not reflect_param_has_default("greet", "nope"):
        print("nope not found")
