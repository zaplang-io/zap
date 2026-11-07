# diff: diverge D-0067: closures capture free variables by value at construction
# Zap captures a closure's free variables by value when the closure is CREATED
# (I#163, D-0067), so rebinding the captured local afterward does not change what
# the closure sees: `f` here returns 1. CPython closures capture by reference
# (late binding), so the same program prints 2. This is a deliberate design
# divergence, pinned so it is noticed if the capture model ever changes.
def main():
    auto n = 1
    auto f = lambda: n
    n = 2
    print(f())
