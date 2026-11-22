# Golden (#882): an arena `cap` overflow raises a first-class MemoryError
# (parent Exception), replacing the arena v1 stopgap RuntimeError. Caught here
# by name; the bound message carries the "MemoryError:" prefix because the
# uncaught reporter renders only the message, never the type name.
def main():
    auto caught = 0
    try:
        with arena(size=64, cap=65536) as a:
            for i in range(0, 100000):
                auto b = a.alloc(bytes, 64)
    except MemoryError as e:
        caught = 1
        print(e)
    print(caught)
