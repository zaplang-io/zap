# Compile-fail fixture (#978, D-0236): heapreplace takes exactly one argument
# (the value to push after popping the root). Calling it with none is rejected.
def main():
    auto h = heap[int64]([1, 2, 3])
    print(h.replace())
