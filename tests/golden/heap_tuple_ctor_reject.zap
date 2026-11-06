# Compile-fail fixture: a heap orders its elements, and a tuple has no defined
# ordering, so `heap[(int64, int64)]()` is rejected cleanly (#703) — the same
# diagnostic as the `heap[(..)]` type annotation, never an internal compiler
# error and no longer the misleading "expects an element type name". (Ordered
# tuple/struct heap elements are a separate follow-up.)
def main():
    auto h = heap[int64, int64]()
    print(0)
