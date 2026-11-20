# Cross-module (#1152): construct an imported struct whose Atomic field
# defaults are generic constructors. Before the fix the field default survived
# monomorphization as a bare `Atomic(...)` and failed codegen with "call to
# unknown function 'Atomic'" in the importing module too.
import conn


def b2i(bool x) -> int64:
    if x:
        return 1
    return 0


def main():
    # Omitted construction: the field defaults evaluate in the importer.
    auto a = conn.Client()
    print(a.state.load())  # 0
    print(b2i(a.closed.load()))  # 0
    print(a.port)  # 11211

    # Per-instance independence across modules too.
    a.state.store(5)
    a.closed.store(True)
    auto b = conn.Client()
    print(a.state.load())  # 5
    print(b.state.load())  # 0  — independent of a
    print(b2i(b.closed.load()))  # 0
