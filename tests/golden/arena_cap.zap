# `cap` overflow raises a catchable error (#553/#882): allocating past the hard
# byte ceiling raises a MemoryError (parent Exception), here caught via
# `except Exception` so the program is deterministic under both collectors.
def main():
    auto caught = 0
    try:
        with arena(size=64, cap=65536) as a:
            for i in range(0, 100000):
                auto b = a.alloc(bytes, 64)
    except Exception:
        caught = 1
    print(caught)
