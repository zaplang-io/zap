# Auto-grow past the `size` hint (#553): a 64-byte size hint rounds up to one
# 32 KiB region; allocating well past that chains more regions transparently.
def main():
    auto total = 0
    with arena(size=64) as a:
        for i in range(0, 5000):
            auto b = a.alloc(bytes, 64)
            total = total + len(b)
    print(total)
