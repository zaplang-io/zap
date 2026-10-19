# Golden fixture: indexing, call, len, and contains overloading (#52, D-0047).
# `a[i]` -> `a.__getitem__(i)`, `a(n)` -> `a.__call__(n)`, `len(a)` ->
# `a.__len__()`, `x in a` -> `a.__contains__(x)`.
struct Ramp:
    int64 base
    int64 size

    def __getitem__(self, int64 i) -> int64:
        return self.base + i

    def __call__(self, int64 n) -> int64:
        return self.base * n

    def __len__(self) -> int64:
        return self.size

    def __contains__(self, int64 v) -> bool:
        return v >= self.base and v < self.base + self.size

def main():
    auto g = Ramp(base=10, size=3)
    print(g[5])
    print(g(4))
    print(len(g))
    if 11 in g:
        print("11 in g")
    if 99 in g:
        print("99 in g")
    else:
        print("99 not in g")
