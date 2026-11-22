# Golden (#882): the arena MemoryError sits under `Exception` in the hierarchy,
# and is a DISTINCT type from `RuntimeError`. (1) `except Exception` catches it.
# (2) An `except RuntimeError` does NOT catch it, so the arena OOM propagates
# out of the inner handler to the enclosing `except MemoryError`.
def main():
    try:
        with arena(size=64, cap=65536) as a:
            for i in range(0, 100000):
                auto b = a.alloc(bytes, 64)
    except Exception:
        print("caught-as-exception")

    try:
        try:
            with arena(size=64, cap=65536) as a2:
                for i in range(0, 100000):
                    auto b2 = a2.alloc(bytes, 64)
        except RuntimeError:
            print("WRONG-caught-by-runtimeerror")
    except MemoryError:
        print("propagated-to-memoryerror")
