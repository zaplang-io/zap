# Golden fixture: the `@deprecated` built-in decorator (I#472, D-0109).
#
# `@deprecated` is a function/method decorator. It generates NO code — the
# decorated body lowers unchanged — and emits a non-fatal use-site *warning*
# (ZAP5124) at each call site during compilation. The warning goes to the
# compiler's stderr, so a deprecated program still builds and runs normally;
# this fixture pins that runtime behavior (its stdout). The warning itself and
# its message variant are covered exhaustively by the sema unit tests.

@deprecated
def old_free() -> int64:
    return 1

@deprecated("use shiny() instead")
def legacy_free() -> int64:
    return 2

struct Widget:
    int64 size

    @deprecated("use area()")
    def legacy_method(self) -> int64:
        return self.size

    def area(self) -> int64:
        return self.size * self.size

def main():
    # Both call sites still resolve and run; deprecation does not change behavior.
    print(old_free())
    print(legacy_free())
    auto w = Widget(size=3)
    print(w.legacy_method())
    print(w.area())
