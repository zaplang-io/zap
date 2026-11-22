# Golden (#882): user-raised `MemoryError` is catchable by name and as its
# parent `Exception`; and the types are distinct in the other direction too — an
# `except MemoryError` does NOT catch a `RuntimeError`, which propagates to the
# enclosing `except RuntimeError`.
def main():
    try:
        raise MemoryError("boom")
    except MemoryError as e:
        print(e)

    try:
        raise MemoryError("boom-as-exception")
    except Exception as e:
        print(e)

    try:
        try:
            raise RuntimeError("runtime-not-memory")
        except MemoryError:
            print("WRONG-memoryerror-caught-runtimeerror")
    except RuntimeError as e:
        print(e)
